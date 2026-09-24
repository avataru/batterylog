//! Golden-value tests for the pool-matching algorithm: pure functions over
//! hand-built fixtures, no database involved.

use std::collections::HashMap;

use batteries_core::db::{Battery, Measurement};
use batteries_core::matching::{self, Draw};

fn battery(id: i64, r#type: &str, brand: &str, nominal_mah: Option<i64>, location: &str) -> Battery {
    Battery {
        id,
        r#type: r#type.to_string(),
        nominal_mah,
        brand: brand.to_string(),
        location: location.to_string(),
        notes: String::new(),
        created_at: String::new(),
        updated_at: String::new(),
        deleted_at: None,
        hard_deleted: None,
    }
}

fn analysis(id: i64, battery_id: i64, measured_at: &str, capacity_mah: i64, ir_mohm: Option<i64>) -> Measurement {
    Measurement {
        id,
        battery_id,
        kind: "analysed".to_string(),
        measured_at: measured_at.to_string(),
        capacity_mah: Some(capacity_mah),
        ir_mohm,
        instrument_id: None,
        mode_id: None,
        discharge_ma: None,
        charge_ma: None,
        notes: String::new(),
        created_at: format!("{measured_at}T00:00:00+00:00"),
    }
}

#[test]
fn eligible_pool_excludes_batteries_with_no_capacity_reading() {
    let batteries = vec![
        battery(1, "AA", "Eneloop", Some(2000), "Storage"),
        battery(2, "AA", "Eneloop", Some(2000), "Storage"),
    ];
    let mut measurements = HashMap::new();
    measurements.insert(1, vec![analysis(1, 1, "2026-01-01", 1900, Some(80))]);
    // Battery 2 has never been measured.

    let pool = matching::eligible_pool(&batteries, &measurements, "storage", 35);
    assert_eq!(pool.len(), 1);
    assert_eq!(pool[0].battery.id, 1);
}

#[test]
fn eligible_pool_matches_location_case_insensitively_and_skips_deleted() {
    let b1 = battery(1, "AA", "Eneloop", Some(2000), "STORAGE");
    let mut b2 = battery(2, "AA", "Eneloop", Some(2000), "Storage");
    b2.deleted_at = Some("2026-01-01T00:00:00Z".to_string());
    let b3 = battery(3, "AA", "Eneloop", Some(2000), "Drawer");
    let batteries = vec![b1, b2, b3];

    let mut measurements = HashMap::new();
    for id in [1, 2, 3] {
        measurements.insert(id, vec![analysis(id, id, "2026-01-01", 1900, Some(80))]);
    }

    let pool = matching::eligible_pool(&batteries, &measurements, "storage", 35);
    let ids: Vec<i64> = pool.iter().map(|c| c.battery.id).collect();
    assert_eq!(ids, vec![1]);
}

#[test]
fn eligible_pool_defaults_ir_and_flags_it_assumed() {
    let batteries = vec![battery(1, "AA", "Eneloop", Some(2000), "Storage")];
    let mut measurements = HashMap::new();
    measurements.insert(1, vec![analysis(1, 1, "2026-01-01", 1900, None)]);

    let pool = matching::eligible_pool(&batteries, &measurements, "storage", 35);
    assert_eq!(pool[0].ir_mohm, 35);
    assert!(pool[0].ir_assumed);
}

#[test]
fn eligible_pool_uses_the_most_recent_capacity_and_ir_readings() {
    let batteries = vec![battery(1, "AA", "Eneloop", Some(2000), "Storage")];
    let mut measurements = HashMap::new();
    measurements.insert(
        1,
        vec![
            analysis(1, 1, "2026-01-01", 1950, Some(90)),
            analysis(2, 1, "2026-03-01", 1800, Some(120)),
        ],
    );

    let pool = matching::eligible_pool(&batteries, &measurements, "storage", 35);
    assert_eq!(pool[0].capacity_mah, 1800);
    assert_eq!(pool[0].ir_mohm, 120);
    assert!(!pool[0].ir_assumed);
}

fn candidate(id: i64, brand: &str, nominal_mah: Option<i64>, capacity_mah: i64, ir_mohm: i64) -> matching::Candidate {
    matching::Candidate {
        battery: battery(id, "AA", brand, nominal_mah, "Storage"),
        capacity_mah,
        ir_mohm,
        ir_assumed: false,
    }
}

#[test]
fn suggest_prefers_a_single_sufficient_bucket_over_mixing() {
    let candidates = vec![
        candidate(1, "Eneloop", Some(2000), 1900, 80),
        candidate(2, "Eneloop", Some(2000), 1850, 90),
        candidate(3, "Duracell", Some(2000), 1950, 70),
    ];
    let suggestion = matching::suggest(&candidates, "AA", 2, Draw::Low).unwrap();
    assert!(!suggestion.mixed);
    assert_eq!(suggestion.battery_ids, vec![1, 2], "results should come back in id order, not rank order");
}

#[test]
fn suggest_mixes_brands_when_no_bucket_has_enough() {
    let candidates = vec![
        candidate(1, "Eneloop", Some(2000), 1900, 80),
        candidate(2, "Duracell", Some(2000), 1950, 70),
    ];
    let suggestion = matching::suggest(&candidates, "AA", 2, Draw::Low).unwrap();
    assert!(suggestion.mixed);
    let mut ids = suggestion.battery_ids;
    ids.sort();
    assert_eq!(ids, vec![1, 2]);
}

#[test]
fn suggest_low_draw_picks_the_weakest_cells() {
    let candidates = vec![
        candidate(1, "Eneloop", Some(2000), 1900, 80),
        candidate(2, "Eneloop", Some(2000), 1200, 200),
        candidate(3, "Eneloop", Some(2000), 1950, 70),
    ];
    let suggestion = matching::suggest(&candidates, "AA", 1, Draw::Low).unwrap();
    assert_eq!(suggestion.battery_ids, vec![2]);
}

#[test]
fn suggest_high_draw_picks_the_strongest_cells() {
    let candidates = vec![
        candidate(1, "Eneloop", Some(2000), 1900, 80),
        candidate(2, "Eneloop", Some(2000), 1200, 200),
        candidate(3, "Eneloop", Some(2000), 1950, 70),
    ];
    let suggestion = matching::suggest(&candidates, "AA", 1, Draw::High).unwrap();
    assert_eq!(suggestion.battery_ids, vec![3]);
}

#[test]
fn suggest_ignores_batteries_of_a_different_type() {
    let candidates = vec![
        candidate(1, "Eneloop", Some(2000), 1900, 80),
        matching::Candidate {
            battery: battery(2, "AAA", "Eneloop", Some(800), "Storage"),
            capacity_mah: 780,
            ir_mohm: 60,
            ir_assumed: false,
        },
    ];
    let err = matching::suggest(&candidates, "AA", 2, Draw::Low).unwrap_err();
    assert!(err.contains("Only 1"));
}

#[test]
fn suggest_rejects_a_request_for_zero() {
    let err = matching::suggest(&[], "AA", 0, Draw::Low).unwrap_err();
    assert!(err.contains("at least one"));
}

#[test]
fn draw_round_trips_through_its_string_form() {
    assert_eq!(Draw::parse("low"), Some(Draw::Low));
    assert_eq!(Draw::parse("high"), Some(Draw::High));
    assert_eq!(Draw::parse("sideways"), None);
    assert_eq!(Draw::Low.as_str(), "low");
    assert_eq!(Draw::High.as_str(), "high");
}
