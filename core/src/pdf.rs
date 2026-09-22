//! A minimal, dependency-free PDF writer for label output — the alternative
//! to sending labels to the network printer (see `label_output` setting).
//!
//! Deliberately hand-rolled rather than pulling in a PDF crate: labels are
//! tiny 1-bit raster images, and a PDF carrying one image per page (or a
//! handful tiled on a page) is a very small, well-defined corner of the PDF
//! spec — an 8-bit grayscale image XObject per label, drawn full-page or at
//! its native size on a grid, no compression, no fonts, no fancy features.
//! Writing that directly avoids taking on an external crate's own dependency
//! surface (in particular, a version-compatibility risk between the popular
//! `printpdf` crate and the `image` crate version this project already
//! depends on, flagged in `printpdf`'s own issue tracker) for what is, at
//! this scope, a few hundred lines of well-understood, testable format.

use crate::labels::Raster;

const PT_PER_MM: f64 = 72.0 / 25.4;
const DOTS_PER_INCH: f64 = 180.0;

fn dots_to_pt(dots: u32) -> f64 {
    dots as f64 / DOTS_PER_INCH * 72.0
}

/// How multiple labels share a PDF. A single label always renders as one
/// page sized exactly to it, regardless of this setting — it only matters
/// once there's more than one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PdfLayout {
    /// One page per label, each page sized exactly to that label (so
    /// printing at 100% scale reproduces the physical label size, the same
    /// way a single label always has).
    Pages,
    /// Labels tiled left-to-right, top-to-bottom on standard A4 pages, for
    /// printing several on one sheet of plain paper.
    Grid,
}

/// Raw bytes to accumulate the PDF into, plus the byte offset of each
/// object written so far (`objects[n]` is where object number `n+1` starts)
/// — PDF object numbers are 1-based, this vec is 0-based to match.
struct PdfWriter {
    buf: Vec<u8>,
    objects: Vec<usize>,
}

impl PdfWriter {
    fn new() -> Self {
        let mut buf = Vec::new();
        buf.extend_from_slice(b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n");
        PdfWriter { buf, objects: Vec::new() }
    }

    /// Starts object `self.objects.len() + 1`, returning that number.
    fn begin_object(&mut self) -> u32 {
        self.objects.push(self.buf.len());
        let n = self.objects.len() as u32;
        self.buf.extend_from_slice(format!("{n} 0 obj\n").as_bytes());
        n
    }

    fn end_object(&mut self) {
        self.buf.extend_from_slice(b"endobj\n");
    }

    fn write(&mut self, s: &str) {
        self.buf.extend_from_slice(s.as_bytes());
    }

    /// Writes a complete `N 0 obj << ... >> stream ... endstream endobj`
    /// object in one call, `dict` being everything between `<<` and `>>`
    /// except `/Length`, which is filled in from `data`.
    fn write_stream_object(&mut self, dict: &str, data: &[u8]) -> u32 {
        let n = self.begin_object();
        self.write(&format!("<< {dict} /Length {} >>\nstream\n", data.len()));
        self.buf.extend_from_slice(data);
        self.write("\nendstream\n");
        self.end_object();
        n
    }

    fn write_dict_object(&mut self, dict: &str) -> u32 {
        let n = self.begin_object();
        self.write(&format!("<< {dict} >>\n"));
        self.end_object();
        n
    }

    /// 8-bit grayscale image XObject: `true` (ink) renders black, matching
    /// `Raster::to_png_bytes`'s convention.
    fn write_image_object(&mut self, label: &Raster) -> u32 {
        let mut data = Vec::with_capacity((label.width * label.height) as usize);
        for y in 0..label.height {
            for x in 0..label.width {
                data.push(if label.get(x, y) { 0u8 } else { 255u8 });
            }
        }
        self.write_stream_object(
            &format!(
                "/Type /XObject /Subtype /Image /Width {} /Height {} \
                 /ColorSpace /DeviceGray /BitsPerComponent 8",
                label.width, label.height
            ),
            &data,
        )
    }

    /// Finishes the file: writes the xref table and trailer, referencing
    /// `catalog` as the document catalog, and returns the finished bytes.
    fn finish(mut self, catalog: u32) -> Vec<u8> {
        let xref_offset = self.buf.len();
        self.write(&format!("xref\n0 {}\n", self.objects.len() + 1));
        self.write("0000000000 65535 f \n");
        let offsets = self.objects.clone();
        for offset in offsets {
            self.write(&format!("{offset:010} 00000 n \n"));
        }
        self.write(&format!(
            "trailer\n<< /Size {} /Root {catalog} 0 R >>\nstartxref\n{xref_offset}\n%%EOF",
            self.objects.len() + 1
        ));
        self.buf
    }
}

/// One page: an image placed at `(x_pt, y_pt)` (from the page's bottom-left,
/// PDF's native origin) at `(w_pt, h_pt)` size, on a page of `page_w_pt` by
/// `page_h_pt`. Multiple images (`Grid` layout) share one page/content
/// stream; `Pages` layout always has exactly one.
struct Placement<'a> {
    label: &'a Raster,
    x_pt: f64,
    y_pt: f64,
    w_pt: f64,
    h_pt: f64,
}

struct Page<'a> {
    width_pt: f64,
    height_pt: f64,
    placements: Vec<Placement<'a>>,
}

fn pages_layout(labels: &[Raster]) -> Vec<Page<'_>> {
    labels
        .iter()
        .map(|label| {
            let w_pt = dots_to_pt(label.width);
            let h_pt = dots_to_pt(label.height);
            Page {
                width_pt: w_pt,
                height_pt: h_pt,
                placements: vec![Placement { label, x_pt: 0.0, y_pt: 0.0, w_pt, h_pt }],
            }
        })
        .collect()
}

/// A4, 10 mm margins, 3 mm gaps between labels, flowing left-to-right and
/// wrapping to a new row (or a new page, if a row wouldn't fit) — a sheet
/// of stickers, not a precision layout.
fn grid_layout(labels: &[Raster]) -> Vec<Page<'_>> {
    const PAGE_W_MM: f64 = 210.0;
    const PAGE_H_MM: f64 = 297.0;
    const MARGIN_MM: f64 = 10.0;
    const GAP_MM: f64 = 3.0;
    let page_w_pt = PAGE_W_MM * PT_PER_MM;
    let page_h_pt = PAGE_H_MM * PT_PER_MM;
    let margin_pt = MARGIN_MM * PT_PER_MM;
    let gap_pt = GAP_MM * PT_PER_MM;

    let fresh_page = || Page { width_pt: page_w_pt, height_pt: page_h_pt, placements: Vec::new() };
    let mut pages: Vec<Page> = vec![fresh_page()];
    let mut cursor_x = margin_pt;
    let mut cursor_top = margin_pt; // distance from the page's top edge
    let mut row_height_pt = 0.0f64;

    for label in labels {
        let w_pt = dots_to_pt(label.width);
        let h_pt = dots_to_pt(label.height);

        if cursor_x + w_pt > page_w_pt - margin_pt && cursor_x > margin_pt {
            // Doesn't fit on the current row — wrap to the next one.
            cursor_x = margin_pt;
            cursor_top += row_height_pt + gap_pt;
            row_height_pt = 0.0;
        }
        if cursor_top + h_pt > page_h_pt - margin_pt && cursor_top > margin_pt {
            // Doesn't fit on the current page — start a new one.
            pages.push(fresh_page());
            cursor_x = margin_pt;
            cursor_top = margin_pt;
            row_height_pt = 0.0;
        }

        let y_pt = page_h_pt - cursor_top - h_pt; // flip to PDF's bottom-up origin
        pages.last_mut().unwrap().placements.push(Placement { label, x_pt: cursor_x, y_pt, w_pt, h_pt });

        cursor_x += w_pt + gap_pt;
        row_height_pt = row_height_pt.max(h_pt);
    }

    pages
}

/// Renders `labels` into a complete PDF file's bytes, one label per page
/// (`Pages`) or several tiled per A4 page (`Grid`). Returns an empty PDF
/// (still a valid, openable zero-page document) if `labels` is empty.
pub fn render_labels_pdf(labels: &[Raster], layout: PdfLayout) -> Vec<u8> {
    let pages = match layout {
        PdfLayout::Pages => pages_layout(labels),
        PdfLayout::Grid => grid_layout(labels),
    };

    let mut w = PdfWriter::new();
    // Object 1 (Catalog) and 2 (Pages) are written first but reference each
    // other and the page objects that come after — PDF object references
    // are just numbers, so objects can point forward to numbers not written
    // yet as long as every number gets written eventually. Object numbers
    // are reserved by writing placeholders isn't needed here: we compute
    // them by knowing the exact count in advance instead.
    let catalog_num = 1u32;
    let pages_num = 2u32;
    let first_page_num = 3u32;

    // Each page needs: 1 page object + 1 content-stream object + 1 image
    // object per placement on it. Numbers are assigned sequentially in that
    // order, page by page, so they can be computed up front.
    let mut page_obj_nums = Vec::with_capacity(pages.len());
    let mut next = first_page_num;
    for page in &pages {
        page_obj_nums.push(next);
        next += 2 + page.placements.len() as u32; // page + content + one image each
    }

    // Reserve objects 1 and 2 (written for real at the end, once the page
    // numbers referenced inside them are known) by writing them as
    // placeholders now and overwriting isn't possible in an append-only
    // writer, so instead: write everything else first, and objects 1/2
    // last — object numbers don't have to be written in numeric order.
    w.objects.push(0); // placeholder offset for object 1, fixed up below
    w.objects.push(0); // placeholder offset for object 2, fixed up below

    for (page, &page_num) in pages.iter().zip(&page_obj_nums) {
        let content_num = page_num + 1;
        let mut xobjects = String::new();
        let mut content = String::new();
        for (i, placement) in page.placements.iter().enumerate() {
            let image_num = content_num + 1 + i as u32;
            xobjects.push_str(&format!("/Im{i} {image_num} 0 R "));
            content.push_str(&format!(
                "q {:.3} 0 0 {:.3} {:.3} {:.3} cm /Im{i} Do Q\n",
                placement.w_pt, placement.h_pt, placement.x_pt, placement.y_pt
            ));
        }

        assert_eq!(w.objects.len() as u32 + 1, page_num, "page object numbering drifted");
        w.write_dict_object(&format!(
            "/Type /Page /Parent {pages_num} 0 R /MediaBox [0 0 {:.3} {:.3}] \
             /Resources << /XObject << {xobjects}>> >> /Contents {content_num} 0 R",
            page.width_pt, page.height_pt
        ));

        assert_eq!(w.objects.len() as u32 + 1, content_num, "content object numbering drifted");
        w.write_stream_object("", content.as_bytes());

        // One image object per placement, numbered content_num+1, +2, ...
        // in placement order — the same numbering `image_num` above assumed.
        for placement in &page.placements {
            w.write_image_object(placement.label);
        }
    }

    // Now that every page number is known, write objects 1 and 2 for real,
    // fixing up the offsets reserved above.
    let kids: String = page_obj_nums.iter().map(|n| format!("{n} 0 R ")).collect();
    w.objects[(catalog_num - 1) as usize] = w.buf.len();
    w.write(&format!("{catalog_num} 0 obj\n<< /Type /Catalog /Pages {pages_num} 0 R >>\nendobj\n"));
    w.objects[(pages_num - 1) as usize] = w.buf.len();
    w.write(&format!(
        "{pages_num} 0 obj\n<< /Type /Pages /Kids [{kids}] /Count {} >>\nendobj\n",
        pages.len()
    ));

    w.finish(catalog_num)
}
