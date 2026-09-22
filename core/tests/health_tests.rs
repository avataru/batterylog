//! Golden-value tests: fixtures with known-correct expected output, checked
//! against `health::summarise` and friends for exact parity.

use std::collections::HashMap;

use batteries_core::db::{Battery, Measurement};
use batteries_core::health::{self, health_color, limit_for};

fn measurement(
    id: i64,
    kind: &str,
    measured_at: &str,
    capacity_mah: Option<i64>,
    ir_mohm: Option<i64>,
    instrument_id: Option<i64>,
    discharge_ma: Option<i64>,
) -> Measurement {
    Measurement {
        id,
        battery_id: 1,
        kind: kind.to_string(),
        measured_at: measured_at.to_string(),
        capacity_mah,
        ir_mohm,
        instrument_id,
        mode_id: None,
        discharge_ma,
        charge_ma: None,
        notes: String::new(),
        created_at: format!("{measured_at}T00:00:00+00:00"),
    }
}

fn battery() -> Battery {
    Battery {
        id: 1,
        r#type: "AA".to_string(),
        nominal_mah: Some(2000),
        brand: "Eneloop".to_string(),
        location: "Drawer".to_string(),
        notes: String::new(),
        created_at: String::new(),
        updated_at: String::new(),
        deleted_at: None,
        hard_deleted: None,
    }
}

fn fixture_measurements() -> Vec<Measurement> {
    vec![
        measurement(1, "bought", "2026-01-01", None, None, None, None),
        measurement(2, "analysed", "2026-02-01", Some(1950), Some(90), Some(10), Some(500)),
        measurement(3, "analysed", "2026-03-01", Some(1900), Some(95), Some(10), Some(500)),
        measurement(4, "analysed", "2026-03-15", Some(1700), Some(150), Some(20), Some(1000)),
        measurement(5, "analysed", "2026-04-01", Some(1850), Some(98), Some(10), Some(500)),
    ]
}

#[test]
fn summarise_matches_python_golden_values() {
    let battery = battery();
    let measurements = fixture_measurements();
    let mut limits = HashMap::new();
    limits.insert("AA".to_string(), 100);
    limits.insert("AAA".to_string(), 200);
    let mut names = HashMap::new();
    names.insert(10, "C9000".to_string());
    names.insert(20, "SkyRC MC3000".to_string());

    let summary = health::summarise(&battery, &measurements, &limits, Some(80), &names);

    assert_eq!(summary.retention, Some(92));
    assert!(!summary.health_low);
    assert_eq!(summary.ir_limit, Some(100));
    assert!(!summary.ir_over_limit);
    assert!(!summary.ir_unchecked);
    assert_eq!(summary.setup, "C9000 at 500 mA");
    assert_eq!(ids(&summary.series), vec![2, 3, 5]);
    assert_eq!(ids(&summary.others), vec![4]);
    assert_eq!(ids(&summary.newer), Vec::<i64>::new());
    assert_eq!(summary.ir_first.as_ref().map(|m| m.id), Some(2));
    assert_eq!(summary.ir_latest.as_ref().map(|m| m.id), Some(5));
    assert_eq!(summary.ir_change, Some(8));

    let chart = summary.chart.expect("chart present");
    assert_eq!(chart.cap_high, 2024);
    assert_eq!(chart.cap_low, 1676);
    assert_eq!(chart.ir_high, 155);
    assert_eq!(chart.ir_low, 85);
    assert_eq!(chart.nominal_y, Some(20.0));
    assert_eq!(chart.series.len(), 4);
    assert_eq!(chart.first, "2026-02-01");
    assert_eq!(chart.last, "2026-04-01");
}

fn ids(measurements: &[Measurement]) -> Vec<i64> {
    measurements.iter().map(|m| m.id).collect()
}

#[test]
fn health_color_matches_python_golden_values() {
    assert_eq!(health_color(None, Some(80)), "#2a2f3d");
    assert_eq!(health_color(Some(100), Some(80)), "hsl(130, 55%, 22%)");
    assert_eq!(health_color(Some(80), Some(80)), "hsl(45, 55%, 22%)");
    assert_eq!(health_color(Some(79), Some(80)), "hsl(44, 55%, 22%)");
    assert_eq!(health_color(Some(40), Some(80)), "hsl(22, 55%, 22%)");
    assert_eq!(health_color(Some(0), Some(80)), "hsl(0, 55%, 22%)");
    assert_eq!(health_color(Some(95), Some(90)), "hsl(88, 55%, 22%)");
}

#[test]
fn limit_for_matches_python_golden_values() {
    let mut limits = HashMap::new();
    limits.insert("AA".to_string(), 100);
    limits.insert("AAA".to_string(), 200);

    let with_type = |t: &str| Battery { r#type: t.to_string(), ..battery() };

    assert_eq!(limit_for(&with_type("AA"), &limits), Some(100));
    assert_eq!(limit_for(&with_type("aa"), &limits), Some(100));
    assert_eq!(limit_for(&with_type("AA (Eneloop)"), &limits), Some(100));
    assert_eq!(limit_for(&with_type("AAA"), &limits), Some(200));
    assert_eq!(limit_for(&with_type("AAA NiMH"), &limits), Some(200));
    assert_eq!(limit_for(&with_type("C"), &limits), None);
    assert_eq!(limit_for(&with_type("18650"), &limits), None);
}
