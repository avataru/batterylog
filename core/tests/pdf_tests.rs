//! Structural self-consistency checks for the hand-rolled PDF writer. There's
//! no real PDF parser here to confirm a file opens correctly, so the next
//! best thing is verifying the xref table's own byte offsets actually point
//! at the objects they claim to — exactly the kind of off-by-one bookkeeping
//! mistake this format is prone to.

use batteries_core::labels::render_datamatrix;
use batteries_core::pdf::{render_labels_pdf, PdfLayout};

/// Parses the xref table out of a PDF's bytes and asserts every listed
/// in-use ("n") offset lands on a line starting with "<N> 0 obj", N being
/// that entry's 1-based object number (entry 0 is always the free-list
/// head and is skipped, matching the spec).
fn assert_xref_offsets_are_correct(pdf: &[u8]) {
    let text = String::from_utf8_lossy(pdf);
    let xref_pos = text.find("\nxref\n").expect("no xref table found") + 1;
    let after_xref = &text[xref_pos..];
    let mut lines = after_xref.lines();
    assert_eq!(lines.next().unwrap(), "xref");
    let subsection = lines.next().unwrap(); // "0 <count>"
    let count: usize = subsection.split_whitespace().nth(1).unwrap().parse().unwrap();
    let _free_list_head = lines.next().unwrap(); // entry 0: "0000000000 65535 f " — not a real object

    for obj_num in 1..count {
        let entry = lines.next().unwrap();
        let offset: usize = entry.split_whitespace().next().unwrap().parse().unwrap();
        let expected_prefix = format!("{obj_num} 0 obj");
        let actual = pdf.get(offset..offset + expected_prefix.len().min(pdf.len() - offset)).unwrap_or(&[]);
        assert_eq!(
            String::from_utf8_lossy(actual),
            expected_prefix,
            "xref offset for object {obj_num} does not point at its own \"N 0 obj\" marker"
        );
    }
}

fn sample_label(id: i64) -> batteries_core::labels::Raster {
    render_datamatrix(&format!("{id:03}"), 3).unwrap()
}

#[test]
fn a_single_label_produces_a_well_formed_one_page_pdf() {
    let labels = vec![sample_label(1)];
    let pdf = render_labels_pdf(&labels, PdfLayout::Pages);

    assert!(pdf.starts_with(b"%PDF-1.4"), "must start with a PDF header");
    assert!(pdf.ends_with(b"%%EOF"), "must end with the PDF end-of-file marker");
    assert_xref_offsets_are_correct(&pdf);
}

#[test]
fn multiple_labels_in_pages_layout_each_get_their_own_page() {
    let labels: Vec<_> = (1..=3).map(sample_label).collect();
    let pdf = render_labels_pdf(&labels, PdfLayout::Pages);

    assert_xref_offsets_are_correct(&pdf);
    let text = String::from_utf8_lossy(&pdf);
    // "/Type /Page " (trailing space) deliberately excludes "/Type /Pages".
    assert_eq!(text.matches("/Type /Page ").count(), 3, "one Page object per label");
    assert_eq!(text.matches("/Type /Pages").count(), 1, "exactly one Pages tree");
}

#[test]
fn grid_layout_packs_several_labels_onto_one_page() {
    let labels: Vec<_> = (1..=4).map(sample_label).collect();
    let pdf = render_labels_pdf(&labels, PdfLayout::Grid);

    assert_xref_offsets_are_correct(&pdf);
    let text = String::from_utf8_lossy(&pdf);
    // Small labels at A4 scale should all fit on a single page.
    assert_eq!(text.matches("/Type /Page ").count(), 1, "should fit on one page");
    assert_eq!(text.matches("/Subtype /Image").count(), 4, "one image per label");
}

#[test]
fn grid_layout_wraps_to_further_pages_once_a_page_is_full() {
    // At 3 dots/module these Data Matrix symbols are only ~5mm square, so
    // it takes a four-figure count to reliably overflow one A4 page's
    // ~190x277mm usable area (confirmed empirically: 200 alone still fit
    // on one page).
    let labels: Vec<_> = (1..=1000).map(sample_label).collect();
    let pdf = render_labels_pdf(&labels, PdfLayout::Grid);

    assert_xref_offsets_are_correct(&pdf);
    let text = String::from_utf8_lossy(&pdf);
    assert!(text.matches("/Type /Page ").count() > 1, "1000 labels must not fit on one A4 page");
    assert_eq!(text.matches("/Subtype /Image").count(), 1000, "every label must still appear exactly once");
}

#[test]
fn an_empty_label_list_still_produces_an_openable_pdf() {
    let pdf = render_labels_pdf(&[], PdfLayout::Pages);
    assert!(pdf.starts_with(b"%PDF-1.4"));
    assert_xref_offsets_are_correct(&pdf);
}
