//! Renders a battery label as a monochrome raster and, when a printer is
//! configured, sends it to a Brother P-touch over the network.
//!
//! The Data Matrix payload is just the battery id, zero-padded to 3 digits
//! (`PAYLOAD_MIN_DIGITS`/`MAX_ID` fix the width so every label comes out
//! the same size).
//!
//! The network print path (`printing` module) implements Brother's raster
//! protocol against their own official "Software Developer's Manual: Raster
//! Command Reference, PT-E550W/P750W/P710BT" (downloaded from
//! download.brother.com, not any GPL prior art). The first version of this
//! module, written from protocol structure alone without that document in
//! hand, connected fine but printed every label blank — traced to two bugs
//! once the real spec was checked line-by-line against the code: raster
//! lines were transposed (rows sent where per-feed-step columns were
//! needed), and lines weren't padded to this printer's fixed 128-pin/16-byte
//! frame with the correct per-tape-width margin offset, so whatever *did*
//! land was likely off the printable area. Both are fixed and confirmed on
//! real hardware: a single label now prints correctly (right size, correct
//! content).
//!
//! A second, separate class of bug showed up specifically on **multi-label**
//! jobs, and took several rounds — each one a real live-hardware test, one
//! of which left the printer needing a physical restart — to actually pin
//! down: only the first couple of labels printed, or wrong ones printed, or
//! the printer cut in the wrong place. Guessing further from the spec's
//! prose alone (which is how the raster-line fix above was found, and it
//! worked) kept producing new, different failures instead of a fix, so this
//! module was finally cross-checked directly against a **known-working**
//! reference: the `ptouch` Python library (LGPL-2.1+; read for protocol
//! structure and behavior only, nothing copied). That comparison found
//! several concrete divergences, now all fixed to match:
//!
//! - **Compression has to be on.** The reference hardcodes TIFF/PackBits
//!   compression for this exact printer family with the comment "required
//!   for cutting to work" — not documented as a requirement anywhere in
//!   Brother's own spec text. This module previously sent explicit
//!   *no*-compression. Raster lines are now PackBits-encoded (`pack_bits`,
//!   with an all-zero-line shortcut, `Z`), and the compression-mode command
//!   selects TIFF instead.
//! - **`ESC i z`'s n9 ("starting page") is always 0.** An earlier fix here
//!   varied it per label position, reasoning from the spec's "Starting
//!   page: 0, Other pages: 1" line alone — the reference never varies it.
//! - **The protocol's real chain-printing bit (`ESC i K` bit 3) is never
//!   actually used**, even for a multi-label job — an earlier version of
//!   this code set it based on whether the job was chained, which is the
//!   wrong mental model entirely. What actually keeps a multi-label job
//!   from fully cutting between labels is **half-cut** (`ESC i K` bit 2)
//!   combined with **auto-cut off** (`ESC i M` bit 6) — matching this app's
//!   own `HALF_CUT` setting, previously a real no-op (see the README's old
//!   "known limitations"), now wired in and load-bearing.
//! - **Two control codes were missing outright**: `ESC i d` (feed margin,
//!   sent every label) and `ESC i A` (cut every 1 label, sent whenever
//!   auto-cut is on) — both on the spec's own "Control codes" list, with no
//!   equivalent here at all before.
//!
//! None of this — the compression requirement especially — is something
//! the spec text itself would have surfaced; it's specific, hard-won
//! knowledge from the reference implementation's own comments. This fix is
//! **not yet hardware-confirmed** — the next real multi-label print is what
//! validates it.

use std::io::{self, Write};
use std::net::TcpStream;
use std::path::PathBuf;

use ab_glyph::{Font, FontRef, Glyph, OutlinedGlyph, PxScale, ScaleFont};
use datamatrix::{DataMatrix, SymbolList};
use image::{ImageBuffer, Luma};
use serde::Serialize;

use crate::config;
use crate::db::Db;

// ── Payload ──────────────────────────────────────────────────────────────

pub const PAYLOAD_MIN_DIGITS: usize = 3;
pub const MAX_ID: i64 = 999;

pub fn encode_payload(battery_id: i64) -> Result<String, String> {
    if battery_id < 1 {
        return Err(format!(
            "A battery id must be a whole number of 1 or more, not {battery_id}."
        ));
    }
    if battery_id > MAX_ID {
        return Err(format!(
            "Battery id {battery_id} is above the limit of {MAX_ID}. Ids are three digits so \
             that every label comes out the same size."
        ));
    }
    Ok(format!("{battery_id:0width$}", width = PAYLOAD_MIN_DIGITS))
}

/// Leading zeros are discarded, so a label reading "007" and someone typing
/// "7" by hand reach the same battery.
pub fn decode_payload(scanned: &str) -> Result<i64, String> {
    let scanned = scanned.trim();
    if scanned.is_empty() {
        return Err("Nothing was scanned.".to_string());
    }
    let digits: String = scanned.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return Err(format!("{scanned:?} does not contain a battery id."));
    }
    digits
        .parse::<i64>()
        .map_err(|_| format!("{scanned:?} does not contain a battery id."))
}

// ── Printer status ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct PrinterStatus {
    pub available: bool,
    pub description: String,
}

pub fn printer_status(db: &Db) -> PrinterStatus {
    let host = config::get(db, "printer_host").as_text();
    if !host.is_empty() {
        return PrinterStatus {
            available: true,
            description: format!("network printer at {host}:9100"),
        };
    }
    if config::get(db, "printer_usb").as_bool() {
        return PrinterStatus {
            available: true,
            description: "USB printer".to_string(),
        };
    }
    PrinterStatus {
        available: false,
        description: "No printer configured. Set the printer address on the Settings page. \
                       Until then you can download each label as a PNG and print it from \
                       P-touch Editor."
            .to_string(),
    }
}

// ── Raster canvas ────────────────────────────────────────────────────────

/// A 1-bit monochrome canvas. `true` = ink (black).
#[derive(Debug, Clone)]
pub struct Raster {
    pub width: u32,
    pub height: u32,
    pixels: Vec<bool>,
}

impl Raster {
    fn blank(width: u32, height: u32) -> Self {
        Raster {
            width,
            height,
            pixels: vec![false; (width * height) as usize],
        }
    }

    fn set(&mut self, x: i64, y: i64, value: bool) {
        if x < 0 || y < 0 || x as u32 >= self.width || y as u32 >= self.height {
            return;
        }
        let idx = y as usize * self.width as usize + x as usize;
        self.pixels[idx] = value;
    }

    pub fn get(&self, x: u32, y: u32) -> bool {
        self.pixels[y as usize * self.width as usize + x as usize]
    }

    fn paste(&mut self, other: &Raster, x0: i64, y0: i64) {
        for y in 0..other.height {
            for x in 0..other.width {
                if other.get(x, y) {
                    self.set(x0 + x as i64, y0 + y as i64, true);
                }
            }
        }
    }

    pub fn to_png_bytes(&self) -> Result<Vec<u8>, String> {
        let mut img: ImageBuffer<Luma<u8>, Vec<u8>> = ImageBuffer::new(self.width, self.height);
        for y in 0..self.height {
            for x in 0..self.width {
                let value = if self.get(x, y) { 0u8 } else { 255u8 };
                img.put_pixel(x, y, Luma([value]));
            }
        }
        let mut bytes: Vec<u8> = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png)
            .map_err(|e| e.to_string())?;
        Ok(bytes)
    }
}

// ── Data Matrix rendering ────────────────────────────────────────────────

const DOTS_PER_MODULE: u32 = 4;
const QUIET_MODULES: u32 = 1;

/// Render the payload as a Data Matrix, including its quiet zone. The
/// `datamatrix` crate's bitmap excludes the quiet zone (confirmed: its
/// `width()`/`height()` docs say "no quiet zone included"), so the quiet
/// zone is added manually here, one module at a time.
pub fn render_datamatrix(payload: &str, dots_per_module: u32) -> Result<Raster, String> {
    let code = DataMatrix::encode(payload.as_bytes(), SymbolList::default())
        .map_err(|e| format!("Data Matrix encoding failed: {e:?}"))?;
    let bitmap = code.bitmap();
    let modules = bitmap.width() as u32 + 2 * QUIET_MODULES;
    let size = modules * dots_per_module;

    let mut raster = Raster::blank(size, size);
    for (x, y) in bitmap.pixels() {
        let x0 = (x as u32 + QUIET_MODULES) * dots_per_module;
        let y0 = (y as u32 + QUIET_MODULES) * dots_per_module;
        for dy in 0..dots_per_module {
            for dx in 0..dots_per_module {
                raster.set((x0 + dx) as i64, (y0 + dy) as i64, true);
            }
        }
    }
    Ok(raster)
}

// ── Font resolution ──────────────────────────────────────────────────────

const BOLD_CANDIDATES: &[&str] = &[
    "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
    "/usr/share/fonts/dejavu/DejaVuSans-Bold.ttf",
    "/usr/share/fonts/TTF/DejaVuSans-Bold.ttf",
    "/Library/Fonts/Arial Bold.ttf",
    "/System/Library/Fonts/Supplemental/Arial Bold.ttf",
    "C:/Windows/Fonts/segoeuib.ttf",
    "C:/Windows/Fonts/arialbd.ttf",
    "C:/Windows/Fonts/calibrib.ttf",
];

fn find_bold_font() -> Result<PathBuf, String> {
    if let Ok(chosen) = std::env::var("FONT_BOLD") {
        let chosen = chosen.trim();
        if !chosen.is_empty() {
            return Ok(PathBuf::from(chosen));
        }
    }
    for candidate in BOLD_CANDIDATES {
        let path = PathBuf::from(candidate);
        if path.exists() {
            return Ok(path);
        }
    }
    Err("No usable bold font was found. Set the FONT_BOLD environment variable to the full \
         path of a TrueType file."
        .to_string())
}

// ── Label composition ────────────────────────────────────────────────────

/// `(left-margin pins, print-area pins)` for a given tape width, out of this
/// printer family's fixed 128-pin head — from Brother's own Raster Command
/// Reference for the PT-E550W/P750W/P710BT ("2.3.5 Raster line", the TZe
/// tape table), not a guess: left + print-area + right always sums to 128,
/// and the right margin always equals the left. Kept as one lookup, rather
/// than two that could drift apart, since `printing::left_margin_pins` and
/// `print_pins` must always agree on which widths exist.
fn head_layout(tape_mm_x10: i64) -> Option<(u32, u32)> {
    match tape_mm_x10 {
        35 => Some((52, 24)),
        60 => Some((48, 32)),
        90 => Some((39, 50)),
        120 => Some((29, 70)),
        180 => Some((8, 112)),
        240 => Some((0, 128)),
        _ => None,
    }
}

/// Printable pixels across the tape at 180 dpi, keyed by tape width in mm
/// times 10 (to use integer keys instead of float ones).
fn print_pins(tape_mm_x10: i64) -> Option<u32> {
    head_layout(tape_mm_x10).map(|(_, print_area)| print_area)
}

const GAP_PX: i64 = 8;
const PAD_PX: i64 = 6;

struct InkBounds {
    min_x: f32,
    min_y: f32,
    max_x: f32,
    max_y: f32,
}

fn layout_glyphs(font: &FontRef, text: &str, scale: PxScale, start_x: f32, start_y: f32) -> Vec<Glyph> {
    let scaled = font.as_scaled(scale);
    let mut caret = start_x;
    let mut glyphs = Vec::new();
    for c in text.chars() {
        let id = font.glyph_id(c);
        glyphs.push(id.with_scale_and_position(scale, ab_glyph::point(caret, start_y)));
        caret += scaled.h_advance(id);
    }
    glyphs
}

fn ink_bounds(font: &FontRef, glyphs: &[Glyph]) -> Option<InkBounds> {
    let mut bounds: Option<InkBounds> = None;
    for glyph in glyphs {
        if let Some(outlined) = font.outline_glyph(glyph.clone()) {
            let b = outlined.px_bounds();
            bounds = Some(match bounds {
                None => InkBounds {
                    min_x: b.min.x,
                    min_y: b.min.y,
                    max_x: b.max.x,
                    max_y: b.max.y,
                },
                Some(acc) => InkBounds {
                    min_x: acc.min_x.min(b.min.x),
                    min_y: acc.min_y.min(b.min.y),
                    max_x: acc.max_x.max(b.max.x),
                    max_y: acc.max_y.max(b.max.y),
                },
            });
        }
    }
    bounds
}

fn draw_glyphs(raster: &mut Raster, font: &FontRef, glyphs: &[Glyph]) {
    for glyph in glyphs {
        if let Some(outlined) = font.outline_glyph(glyph.clone()) {
            draw_outlined(raster, &outlined);
        }
    }
}

fn draw_outlined(raster: &mut Raster, outlined: &OutlinedGlyph) {
    let bounds = outlined.px_bounds();
    outlined.draw(|gx, gy, coverage| {
        if coverage > 0.5 {
            let px = bounds.min.x as i64 + gx as i64;
            let py = bounds.min.y as i64 + gy as i64;
            raster.set(px, py, true);
        }
    });
}

/// Compose the label: the Data Matrix on the left, the battery id beside
/// it. Vertical centering of the id text uses the glyphs' actual ink
/// bounding box (via `ab_glyph`'s outline bounds) rather than the font's
/// declared metrics, which include ascender/descender space the digits
/// don't use and would otherwise leave the text sitting visibly off-centre.
pub fn render_label(battery_id: i64, tape_mm_x10: i64, id_text_scale: f64) -> Result<Raster, String> {
    let height = print_pins(tape_mm_x10).ok_or_else(|| {
        format!(
            "Tape width {} mm is not one this printer supports.",
            tape_mm_x10 as f64 / 10.0
        )
    })?;

    let payload = encode_payload(battery_id)?;
    let mut symbol = render_datamatrix(&payload, DOTS_PER_MODULE)?;
    if symbol.height > height {
        for dots in (2..DOTS_PER_MODULE).rev() {
            symbol = render_datamatrix(&payload, dots)?;
            if symbol.height <= height {
                break;
            }
        }
    }

    let font_path = find_bold_font()?;
    let font_bytes = std::fs::read(&font_path).map_err(|e| e.to_string())?;
    let font = FontRef::try_from_slice(&font_bytes).map_err(|e| e.to_string())?;

    let id_text = format!("{battery_id:03}");
    let size_px = (height as f64 * id_text_scale).round().max(8.0) as f32;
    let scale = PxScale::from(size_px);

    // First pass at the origin, purely to measure the ink bbox.
    let probe_glyphs = layout_glyphs(&font, &id_text, scale, 0.0, 0.0);
    let bounds = ink_bounds(&font, &probe_glyphs).ok_or("no glyphs to draw")?;
    let text_width = (bounds.max_x - bounds.min_x).round() as i64;

    let width = PAD_PX + symbol.width as i64 + GAP_PX + text_width + PAD_PX;
    let mut raster = Raster::blank(width.max(1) as u32, height);
    raster.paste(&symbol, PAD_PX, (height as i64 - symbol.height as i64) / 2);

    let draw_x = (PAD_PX + symbol.width as i64 + GAP_PX) as f32 - bounds.min_x;
    let draw_y = (height as f32 - (bounds.max_y - bounds.min_y)) / 2.0 - bounds.min_y;
    let glyphs = layout_glyphs(&font, &id_text, scale, draw_x, draw_y);
    draw_glyphs(&mut raster, &font, &glyphs);

    Ok(raster)
}

// ── Printing ─────────────────────────────────────────────────────────────

pub mod printing {
    use super::*;

    /// Total print-head pins for this printer family (PT-E550W/P750W/
    /// P710BT) — always 128 pins / 16 bytes per raster line regardless of
    /// the tape width loaded, per the Raster Command Reference's "2.3.5
    /// Raster line". A narrower tape's print area sits centered within this
    /// fixed frame, offset by `head_layout`'s left-margin pin count; the
    /// rest of each line is left at zero.
    const HEAD_BYTES: usize = 16;

    /// `ESC i d`'s default feed margin, in dots — 2 mm at 180 dpi
    /// (`round(2.0 * 180 / 25.4)`). Matches the known-working reference
    /// implementation's own default; not exposed as a setting since nothing
    /// about this app's usage needs it to vary.
    const DEFAULT_MARGIN_DOTS: u16 = 14;

    /// Brother raster-protocol byte stream over a raw TCP socket to port
    /// 9100 — see this module's file-level doc comment for what's been
    /// fixed against the official spec and what's still unconfirmed on real
    /// hardware.
    pub struct NetworkPrinter {
        stream: TcpStream,
    }

    impl NetworkPrinter {
        pub fn connect(host: &str) -> io::Result<Self> {
            let stream = TcpStream::connect((host, 9100))?;
            Ok(NetworkPrinter { stream })
        }

        fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
            self.stream.write_all(bytes)
        }

        /// Reset the printer's command state: 200 zero bytes (clears any
        /// partial command a previous, possibly-aborted job left behind),
        /// then ESC @ (initialize). Sent once at the start of a job.
        fn initialize(&mut self) -> io::Result<()> {
            self.write_all(&[0u8; 200])?;
            self.write_all(&[0x1b, 0x40])
        }

        /// Switch into raster graphics transfer mode: ESC i a 01. The spec
        /// lists this among the "control codes" sent at the start of every
        /// page/label, not just once per job, so it's called once per label
        /// here too rather than hoisted above the label loop.
        fn enter_raster_mode(&mut self) -> io::Result<()> {
            self.write_all(&[0x1b, 0x69, 0x61, 0x01])
        }

        /// ESC i z: print information command. Tells the printer the tape
        /// width and how many raster lines this label will carry, so it can
        /// confirm the loaded media matches (returning an error status if
        /// not) rather than silently printing against a mismatched or
        /// unknown tape. Flags used are `PI_RECOVER | PI_WIDTH | PI_LENGTH`
        /// (0x86) — matched against a known-working Rust-incompatible
        /// reference implementation of this exact protocol (LGPL-2.1+,
        /// read for protocol structure only, not copied from), not
        /// re-derived from the spec text alone; that reference also
        /// deliberately does *not* assert `PI_KIND` (media type), for the
        /// same reason noted here previously: this app has no reliable way
        /// to know whether the loaded TZe tape is laminated. n9 ("starting
        /// page") is always 0 — an earlier attempt varied it per label
        /// position, reasoning from the spec's "Starting page: 0, Other
        /// pages: 1" line alone, and a live multi-label test came out
        /// wrong; the reference never varies it, so this reverts to
        /// matching that.
        fn print_information(&mut self, tape_mm: u8, raster_count: u32) -> io::Result<()> {
            self.write_all(&Self::print_information_bytes(tape_mm, raster_count))
        }

        fn print_information_bytes(tape_mm: u8, raster_count: u32) -> [u8; 13] {
            const PI_LENGTH: u8 = 0x02;
            const PI_WIDTH: u8 = 0x04;
            const PI_RECOVER: u8 = 0x80;
            let n = raster_count.to_le_bytes();
            [
                0x1b, 0x69, 0x7a,
                PI_RECOVER | PI_WIDTH | PI_LENGTH,
                0x00, // media type: not asserted
                tape_mm,
                0x00, // media length: always 0, continuous tape
                n[0], n[1], n[2], n[3],
                0x00, // starting page: always 0, see doc comment above
                0x00, // fixed
            ]
        }

        /// ESC i M: various mode settings. Bit 6 is auto cut, bit 7 is
        /// mirror printing (always off here).
        fn various_mode_settings(&mut self, auto_cut: bool) -> io::Result<()> {
            self.write_all(&Self::various_mode_settings_bytes(auto_cut))
        }

        fn various_mode_settings_bytes(auto_cut: bool) -> [u8; 4] {
            const AUTO_CUT: u8 = 0x40;
            [0x1b, 0x69, 0x4d, if auto_cut { AUTO_CUT } else { 0x00 }]
        }

        /// ESC i K: advanced mode settings. Bit 2 is half cut, bit 3 is "no
        /// chain printing". Bit 3 is **always set** (chain printing *off*)
        /// here — the reference implementation never actually uses the
        /// protocol's chain-printing bit, even for a multi-label job; an
        /// earlier version of this code set it based on whether the job
        /// was chained, which turned out to be the wrong mental model (see
        /// this module's file-level doc comment). What actually keeps a
        /// multi-label job from cutting fully between labels is `half_cut`
        /// combined with `auto_cut` (in `various_mode_settings`) being
        /// *off* — this is what `HALF_CUT`'s Settings-page help text means
        /// by "that still chains".
        fn advanced_mode_settings(&mut self, half_cut: bool) -> io::Result<()> {
            self.write_all(&Self::advanced_mode_settings_bytes(half_cut))
        }

        fn advanced_mode_settings_bytes(half_cut: bool) -> [u8; 4] {
            const HALF_CUT: u8 = 0x04;
            const NO_CHAIN_PRINTING: u8 = 0x08;
            let n1 = NO_CHAIN_PRINTING | if half_cut { HALF_CUT } else { 0x00 };
            [0x1b, 0x69, 0x4b, n1]
        }

        /// ESC i d: specify the feed margin, in dots.
        fn margin(&mut self, dots: u16) -> io::Result<()> {
            let n = dots.to_le_bytes();
            self.write_all(&[0x1b, 0x69, 0x64, n[0], n[1]])
        }

        /// ESC i A: cut every `pages` labels — always 1 here (cut every
        /// label auto-cut applies to). Only sent when auto-cut is on, per
        /// the reference implementation.
        fn page_number_cuts(&mut self, pages: u8) -> io::Result<()> {
            self.write_all(&[0x1b, 0x69, 0x41, pages])
        }

        /// M 02h: TIFF/PackBits compression. This printer family needs
        /// compression on for its cutter to work at all — confirmed from
        /// the reference implementation's own comment ("required for
        /// cutting to work"), not documented as a requirement anywhere in
        /// Brother's own spec text. An earlier version of this code
        /// explicitly selected *no* compression, which the reference never
        /// does for this printer family.
        fn select_compression(&mut self) -> io::Result<()> {
            self.write_all(&[b'M', 0x02])
        }

        /// One raster line — a single step along the tape's feed direction,
        /// carrying the pins across the print head at that instant.
        /// `Z` (a single byte) is a shortcut for an all-zero line; anything
        /// else is PackBits-compressed and sent as `'G' <len, u16 LE>
        /// <compressed bytes>` — both per the spec's raster-data commands,
        /// used because compression has to be on (see `select_compression`).
        fn send_raster_line(&mut self, packed: &[u8; HEAD_BYTES]) -> io::Result<()> {
            if packed.iter().all(|&b| b == 0) {
                return self.write_all(&[b'Z']);
            }
            let compressed = pack_bits(packed);
            let len = (compressed.len() as u16).to_le_bytes();
            self.write_all(&[b'G', len[0], len[1]])?;
            self.write_all(&compressed)
        }

        /// Sends a label's print command — Control-Z (0x1A, "print with
        /// feeding") when `eject` is true, for the last or only label of a
        /// run; FF (0x0C, "print", used at the end of every page other than
        /// the last) otherwise, for a label in the middle of a chained
        /// batch. No read follows: see this module's file-level doc
        /// comment for why a network connection doesn't wait for a status
        /// reply here the way a USB one would.
        fn print_command(&mut self, eject: bool) -> io::Result<()> {
            self.write_all(&[if eject { 0x1a } else { 0x0c }])
        }

        /// Packs one raster line for `label` at feed-step `x`: a *vertical*
        /// slice of the label image (one bit per pin, `y` in `0..label
        /// .height`), not a horizontal row. `label`'s own coordinate space
        /// has `x` running along the tape's length and `y` across its width
        /// (see `render_label`), which is exactly backwards from how a
        /// single transmitted raster line is defined by the protocol — a
        /// line is one feed step (one `x`), carrying every pin across the
        /// head for that step. Sending image rows as if they were per-feed-
        /// step lines (the original bug) transposed the whole label and,
        /// combined with not padding to the head's fixed 128-pin frame,
        /// meant nothing usable ever reached the visible print area.
        fn packed_raster_line(label: &Raster, x: u32, margin_pins: u32) -> [u8; HEAD_BYTES] {
            let mut out = [0u8; HEAD_BYTES];
            for y in 0..label.height {
                if label.get(x, y) {
                    let pin = margin_pins + y;
                    out[(pin / 8) as usize] |= 0x80 >> (pin % 8);
                }
            }
            out
        }

        /// `chain`: whether this job is a multi-label batch (`labels.len()
        /// > 1 && chain_labels` at the call site). `half_cut`: the app's
        /// `half_cut` setting. Only meaningful when `chain` is true —
        /// matching the reference implementation, a job that isn't chained
        /// always auto-cuts and half-cuts every label (the printer's own
        /// sane default for a single, standalone label), regardless of the
        /// `half_cut` setting; a chained job instead ties auto-cut to
        /// *not* half-cutting (`HALF_CUT`'s help text: turning half-cut off
        /// makes it "cut all the way through" instead — still one job).
        pub fn print_labels(&mut self, labels: &[Raster], tape_mm_x10: i64, chain: bool, half_cut: bool) -> io::Result<()> {
            let (margin_pins, print_area_pins) = head_layout(tape_mm_x10)
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "unsupported tape width"))?;
            let tape_mm = (tape_mm_x10 / 10) as u8;

            self.initialize()?;
            let last = labels.len().saturating_sub(1);
            for (i, label) in labels.iter().enumerate() {
                debug_assert_eq!(label.height, print_area_pins, "label rendered for a different tape width");
                let (auto_cut, half_cut) = if chain { (!half_cut, half_cut) } else { (true, true) };
                self.enter_raster_mode()?;
                self.print_information(tape_mm, label.width)?;
                self.various_mode_settings(auto_cut)?;
                if auto_cut {
                    self.page_number_cuts(1)?;
                }
                self.advanced_mode_settings(half_cut)?;
                self.margin(DEFAULT_MARGIN_DOTS)?;
                self.select_compression()?;
                for x in 0..label.width {
                    self.send_raster_line(&Self::packed_raster_line(label, x, margin_pins))?;
                }
                self.print_command(i == last || !chain)?;
            }
            Ok(())
        }
    }

    /// TIFF/PackBits encoding of one raster line. Standard PackBits: a
    /// repeated-byte run (2 or more identical bytes) becomes a control byte
    /// `257 - run_length` followed by that one byte; a run of non-repeating
    /// bytes becomes a control byte `run_length - 1` followed by those
    /// bytes verbatim. Lines here are always exactly `HEAD_BYTES` (16)
    /// long, far under PackBits' 128-byte run-length limit, so neither run
    /// kind ever needs to split here.
    fn pack_bits(data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < data.len() {
            let run = data[i..].iter().take_while(|&&b| b == data[i]).count();
            if run >= 2 {
                out.push((257 - run) as u8);
                out.push(data[i]);
                i += run;
                continue;
            }
            let start = i;
            let mut len = 1;
            i += 1;
            while i < data.len() {
                let next_run = data[i..].iter().take_while(|&&b| b == data[i]).count();
                if next_run >= 2 {
                    break;
                }
                len += 1;
                i += 1;
            }
            out.push((len - 1) as u8);
            out.extend_from_slice(&data[start..start + len]);
        }
        out
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn head_layout_pins_always_sum_to_128() {
            for tape_mm_x10 in [35, 60, 90, 120, 180, 240] {
                let (margin, print_area) = head_layout(tape_mm_x10).unwrap();
                assert_eq!(margin * 2 + print_area, 128, "tape {tape_mm_x10}: margins + print area must fill the head");
            }
        }

        /// A single lit pixel at the start of the print area (x=0, y=0)
        /// must land at bit `margin_pins` of the 128-bit frame — this is
        /// the exact case the original bug got wrong: no margin offset at
        /// all, so pixels landed at bit 0 regardless of tape width.
        #[test]
        fn a_pixel_at_the_print_areas_edge_lands_at_the_margin_offset() {
            let (margin_pins, print_area_pins) = head_layout(90).unwrap(); // 9 mm: margin 39, print area 50
            let mut label = Raster::blank(1, print_area_pins);
            label.set(0, 0, true);

            let packed = NetworkPrinter::packed_raster_line(&label, 0, margin_pins);

            let expected_byte = (margin_pins / 8) as usize;
            let expected_bit = 0x80u8 >> (margin_pins % 8);
            for (i, byte) in packed.iter().enumerate() {
                if i == expected_byte {
                    assert_eq!(*byte, expected_bit, "the lit pixel should set exactly bit {margin_pins} (byte {expected_byte})");
                } else {
                    assert_eq!(*byte, 0, "every other byte should stay zero");
                }
            }
        }

        /// One raster *line* must be a vertical slice of the label (one bit
        /// per pin, at a fixed feed step `x`), not a row — the original bug
        /// sent rows, which is exactly what this pins down: a pixel at
        /// (x=2, y=3) must show up when packing column x=2, not row y=3.
        #[test]
        fn a_raster_line_is_a_column_of_the_label_not_a_row() {
            let (margin_pins, print_area_pins) = head_layout(90).unwrap();
            let mut label = Raster::blank(5, print_area_pins);
            label.set(2, 3, true);

            let column_2 = NetworkPrinter::packed_raster_line(&label, 2, margin_pins);
            let column_0 = NetworkPrinter::packed_raster_line(&label, 0, margin_pins);

            let pin = margin_pins + 3;
            let bit_set = column_2[(pin / 8) as usize] & (0x80 >> (pin % 8)) != 0;
            assert!(bit_set, "column x=2 should carry the pixel at y=3");
            assert_eq!(column_0, [0u8; HEAD_BYTES], "column x=0 has no lit pixel and must stay all zero");
        }

        /// Regression: an earlier version of this code varied n9 ("starting
        /// page") per label position, reasoning from the spec's prose
        /// alone. A live multi-label print came out wrong after that
        /// change, and the known-working reference implementation never
        /// varies this byte — it's always 0. Pinned down here so it can't
        /// quietly drift back to varying.
        #[test]
        fn print_information_always_claims_starting_page_regardless_of_position() {
            let bytes = NetworkPrinter::print_information_bytes(9, 100);
            assert_eq!(bytes[11], 0x00, "n9 (starting page) must always be 0, per the reference implementation");
        }

        #[test]
        fn various_mode_settings_sets_only_the_auto_cut_bit() {
            assert_eq!(NetworkPrinter::various_mode_settings_bytes(true)[3], 0x40);
            assert_eq!(NetworkPrinter::various_mode_settings_bytes(false)[3], 0x00);
        }

        /// "No chain printing" (bit 3) must always be set — the reference
        /// implementation never uses the protocol's real chain-printing bit,
        /// even for a multi-label job (see this module's file-level doc
        /// comment). Half cut (bit 2) is the only bit that varies here.
        #[test]
        fn advanced_mode_settings_always_declares_no_chain_printing() {
            assert_eq!(NetworkPrinter::advanced_mode_settings_bytes(false)[3], 0x08, "half_cut off: only the no-chain-printing bit");
            assert_eq!(NetworkPrinter::advanced_mode_settings_bytes(true)[3], 0x0c, "half_cut on: no-chain-printing + half-cut bits");
        }

        /// A minimal from-the-spec PackBits decoder, used only to verify
        /// `pack_bits` round-trips correctly — this is what actually
        /// matters (the printer's own decoder just needs valid PackBits,
        /// not byte-for-byte the same encoding a particular encoder would
        /// produce).
        fn unpack_bits(data: &[u8]) -> Vec<u8> {
            let mut out = Vec::new();
            let mut i = 0;
            while i < data.len() {
                let control = data[i] as i8;
                i += 1;
                if control >= 0 {
                    let len = control as usize + 1;
                    out.extend_from_slice(&data[i..i + len]);
                    i += len;
                } else if control != -128 {
                    let count = 1 - control as i32;
                    out.extend(std::iter::repeat(data[i]).take(count as usize));
                    i += 1;
                }
            }
            out
        }

        // send_raster_line special-cases an all-zero line as 'Z', so this
        // uses a non-zero repeated byte instead.
        #[test]
        fn pack_bits_round_trips_an_all_repeated_line() {
            let repeated = [0x5au8; HEAD_BYTES];
            assert_eq!(unpack_bits(&pack_bits(&repeated)), repeated);
        }

        #[test]
        fn pack_bits_round_trips_all_distinct_bytes() {
            let mut distinct = [0u8; HEAD_BYTES];
            for (i, b) in distinct.iter_mut().enumerate() {
                *b = i as u8 * 15 + 1; // no two equal, none zero, stays under 256
            }
            assert_eq!(unpack_bits(&pack_bits(&distinct)), distinct);
        }

        #[test]
        fn pack_bits_round_trips_a_mix_of_runs_and_literals() {
            let mixed: [u8; HEAD_BYTES] = [1, 1, 1, 2, 3, 4, 4, 4, 4, 4, 4, 5, 6, 7, 7, 9];
            assert_eq!(unpack_bits(&pack_bits(&mixed)), mixed);
        }
    }
}

/// Print one label per battery id over a single connection. Renders every
/// label before opening the connection, so a bad battery id fails before
/// any tape is spooled.
pub fn print_labels(db: &Db, battery_ids: &[i64], tape_mm_x10: i64) -> Result<(), String> {
    if battery_ids.is_empty() {
        return Ok(());
    }
    let status = printer_status(db);
    if !status.available {
        return Err(status.description);
    }

    let id_text_scale = config::get(db, "id_text_scale").as_f64();
    let labels: Vec<Raster> = battery_ids
        .iter()
        .map(|&id| render_label(id, tape_mm_x10, id_text_scale))
        .collect::<Result<_, _>>()?;

    let host = config::get(db, "printer_host").as_text();
    if host.is_empty() {
        return Err("USB printing is not implemented yet; configure a network printer.".to_string());
    }
    let chain = labels.len() > 1 && config::get(db, "chain_labels").as_bool();
    let half_cut = config::get(db, "half_cut").as_bool();
    let mut printer = printing::NetworkPrinter::connect(&host).map_err(|e| e.to_string())?;
    printer.print_labels(&labels, tape_mm_x10, chain, half_cut).map_err(|e| e.to_string())
}
