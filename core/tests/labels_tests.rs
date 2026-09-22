use batteries_core::labels::{decode_payload, encode_payload, render_datamatrix, MAX_ID};

#[test]
fn payload_round_trips_through_zero_padding() {
    assert_eq!(encode_payload(7).unwrap(), "007");
    assert_eq!(decode_payload("007").unwrap(), 7);
    assert_eq!(decode_payload("7").unwrap(), 7);
    assert_eq!(decode_payload("  012 ").unwrap(), 12);
}

#[test]
fn encode_payload_rejects_out_of_range_ids() {
    assert!(encode_payload(0).is_err());
    assert!(encode_payload(-1).is_err());
    assert!(encode_payload(MAX_ID + 1).is_err());
    assert!(encode_payload(MAX_ID).is_ok());
}

#[test]
fn decode_payload_rejects_input_with_no_digits() {
    assert!(decode_payload("").is_err());
    assert!(decode_payload("abc").is_err());
}

#[test]
fn datamatrix_renders_a_square_symbol_with_quiet_zone() {
    let raster = render_datamatrix("007", 4).unwrap();
    // Square, and strictly larger than the bare (no-quiet-zone) symbol would
    // be, since a 1-module quiet zone is added on every side.
    assert_eq!(raster.width, raster.height);
    assert!(raster.width % 4 == 0);
    // The quiet zone itself must be blank.
    for x in 0..raster.width {
        assert!(!raster.get(x, 0), "top quiet zone row should be blank");
    }
    for y in 0..raster.height {
        assert!(!raster.get(0, y), "left quiet zone column should be blank");
    }
}
