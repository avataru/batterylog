//! `cargo run -p batteries-core --example render_label <id> <tape_mm_x10> <out.png>`
//! Quick visual sanity-check for the label rasterizer, no printer needed.
use batteries_core::labels::render_label;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let id: i64 = args.get(1).map(|s| s.parse().unwrap()).unwrap_or(7);
    let tape_mm_x10: i64 = args.get(2).map(|s| s.parse().unwrap()).unwrap_or(90);
    let out = args.get(3).cloned().unwrap_or_else(|| "label.png".to_string());

    let raster = render_label(id, tape_mm_x10, 0.40).expect("render failed");
    std::fs::write(&out, raster.to_png_bytes().expect("png encode failed")).expect("write failed");
    println!("wrote {out} ({}x{})", raster.width, raster.height);
}
