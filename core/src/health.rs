//! What a battery's measurements mean.
//!
//! Everything here is a pure function of a battery row and its list of
//! readings — no DB access.

use std::collections::HashMap;

use chrono::NaiveDate;
use serde::Serialize;

use crate::db::{Battery, Measurement};

type Setup = (Option<i64>, Option<i64>);

/// Round-half-to-even ("banker's rounding"), not round-half-away-from-zero
/// like Rust's `f64::round()` — the health percentages in
/// `health_tests.rs`'s golden-value tests depend on exact `.5` ties
/// rounding this way.
fn round_half_even(x: f64) -> i64 {
    let floor = x.floor();
    let diff = x - floor;
    let floor_i = floor as i64;
    if diff < 0.5 {
        floor_i
    } else if diff > 0.5 {
        floor_i + 1
    } else if floor_i % 2 == 0 {
        floor_i
    } else {
        floor_i + 1
    }
}

fn as_date(text: &str) -> Option<NaiveDate> {
    let head = if text.len() >= 10 { &text[..10] } else { text };
    NaiveDate::parse_from_str(head, "%Y-%m-%d").ok()
}

/// Readings that produced a capacity, oldest first.
pub fn analyses(measurements: &[Measurement]) -> Vec<&Measurement> {
    let mut found: Vec<&Measurement> = measurements
        .iter()
        .filter(|m| m.kind == "analysed" && m.capacity_mah.unwrap_or(0) != 0)
        .collect();
    found.sort_by(|a, b| (&a.measured_at, a.id).cmp(&(&b.measured_at, b.id)));
    found
}

/// The same instrument at the same discharge rate — deliberately not the
/// procedure, since on the C9000 several procedures end at the set rate and
/// that rate is what decides the number.
pub fn setup_of(m: &Measurement) -> Setup {
    (m.instrument_id, m.discharge_ma)
}

pub fn describe_setup(setup: Setup, instrument_names: &HashMap<i64, String>) -> String {
    let (instrument_id, rate) = setup;
    let where_ = match instrument_id {
        Some(id) => instrument_names
            .get(&id)
            .cloned()
            .unwrap_or_else(|| "a deleted instrument".to_string()),
        None => "an unnamed instrument".to_string(),
    };
    match rate {
        Some(r) if r != 0 => format!("{where_} at {r} mA"),
        _ => format!("{where_}, rate not recorded"),
    }
}

fn readings_with_ir(measurements: &[Measurement]) -> Vec<&Measurement> {
    let mut found: Vec<&Measurement> = measurements
        .iter()
        .filter(|m| m.ir_mohm.unwrap_or(0) != 0)
        .collect();
    found.sort_by(|a, b| (&a.measured_at, a.id).cmp(&(&b.measured_at, b.id)));
    found
}

/// The setup a cell's capacity trend belongs to: the one with the most
/// readings (not the most recent one), ties broken by whichever setup holds
/// the newer reading.
pub fn dominant_setup(found: &[&Measurement]) -> Option<Setup> {
    if found.is_empty() {
        return None;
    }
    let mut groups: HashMap<Setup, Vec<&Measurement>> = HashMap::new();
    for m in found {
        groups.entry(setup_of(m)).or_default().push(m);
    }
    groups
        .keys()
        .copied()
        .max_by_key(|s| {
            let group = &groups[s];
            let last = group.last().unwrap();
            (group.len(), last.measured_at.clone(), last.id)
        })
}

/// A background colour for a health percentage: red at 0%, green at 100%,
/// with `health_limit` treated as the ramp's midpoint rather than a fixed
/// 50%. `None` (no measured health) gets a neutral grey.
pub fn health_color(retention: Option<i64>, health_limit: Option<i64>) -> String {
    let retention = match retention {
        Some(r) => r,
        None => return "#2a2f3d".to_string(),
    };
    let limit = health_limit.filter(|&l| l != 0).unwrap_or(80) as f64;
    let pct = retention.clamp(0, 100) as f64;
    let hue = if pct >= limit {
        let span = (100.0 - limit).max(1.0);
        let share = (pct - limit) / span;
        45.0 + share * (130.0 - 45.0)
    } else {
        let share = pct / limit.max(1.0);
        share * 45.0
    };
    format!("hsl({:.0}, 55%, 22%)", hue)
}

/// The resistance limit for a cell of this type, tried as the whole type
/// first (so "AA (Eneloop)" matches "AA"), then its leading word with
/// non-alphanumerics stripped (so "AAA" is never mistaken for "AA").
pub fn limit_for(battery: &Battery, limits: &HashMap<String, i64>) -> Option<i64> {
    let written = battery.r#type.trim().to_uppercase();
    if let Some(&v) = limits.get(&written) {
        return Some(v);
    }
    let leading: String = written
        .split(' ')
        .next()
        .unwrap_or("")
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect();
    limits.get(&leading).copied()
}

#[derive(Debug, Clone, Serialize)]
pub struct Summary {
    pub retention: Option<i64>,
    pub health_limit: Option<i64>,
    pub health_low: bool,
    pub ir_limit: Option<i64>,
    pub ir_over_limit: bool,
    pub ir_unchecked: bool,
    pub unavailable: String,
    pub latest: Option<Measurement>,
    pub setup: String,
    pub series: Vec<Measurement>,
    pub others: Vec<Measurement>,
    pub newer: Vec<Measurement>,
    pub ir_first: Option<Measurement>,
    pub ir_latest: Option<Measurement>,
    pub ir_change: Option<i64>,
    pub chart: Option<Chart>,
}

/// The condition of one cell, as far as its readings can say. Health is
/// capacity retention only — the latest measured capacity in the dominant
/// series as a percentage of nominal — deliberately never blended with
/// resistance, which is reported separately against the cell's own first
/// reading.
pub fn summarise(
    battery: &Battery,
    measurements: &[Measurement],
    limits: &HashMap<String, i64>,
    health_limit: Option<i64>,
    instrument_names: &HashMap<i64, String>,
) -> Summary {
    let found = analyses(measurements);
    let (setup, series, others, latest): (
        Option<Setup>,
        Vec<Measurement>,
        Vec<Measurement>,
        Option<Measurement>,
    ) = if !found.is_empty() {
        let setup = dominant_setup(&found);
        let series: Vec<Measurement> = found
            .iter()
            .filter(|m| Some(setup_of(m)) == setup)
            .map(|&m| m.clone())
            .collect();
        let others: Vec<Measurement> = found
            .iter()
            .filter(|m| Some(setup_of(m)) != setup)
            .map(|&m| m.clone())
            .collect();
        let latest = series.last().cloned();
        (setup, series, others, latest)
    } else {
        (None, Vec::new(), Vec::new(), None)
    };

    let newer: Vec<Measurement> = match &latest {
        Some(latest) => others
            .iter()
            .filter(|m| (&m.measured_at, m.id) > (&latest.measured_at, latest.id))
            .cloned()
            .collect(),
        None => Vec::new(),
    };

    let (retention, unavailable) = if latest.is_none() {
        (
            None,
            "No analysis has been recorded yet. Health is the measured capacity as a \
             percentage of the nominal one, so it needs a discharge measurement to exist."
                .to_string(),
        )
    } else if battery.nominal_mah.unwrap_or(0) == 0 {
        (
            None,
            "This battery has no nominal capacity recorded, so there is nothing to measure \
             the analysis against. Add one under Details."
                .to_string(),
        )
    } else {
        let capacity = latest.as_ref().unwrap().capacity_mah.unwrap_or(0) as f64;
        let nominal = battery.nominal_mah.unwrap() as f64;
        (Some(round_half_even(capacity / nominal * 100.0)), String::new())
    };

    let resistances = readings_with_ir(measurements);
    let ir_first = resistances.first().map(|&m| m.clone());
    let ir_latest = resistances.last().map(|&m| m.clone());
    let ir_change = match (&ir_first, &ir_latest) {
        (Some(first), Some(latest)) if first.id != latest.id => {
            Some(latest.ir_mohm.unwrap_or(0) - first.ir_mohm.unwrap_or(0))
        }
        _ => None,
    };

    let limit = limit_for(battery, limits);
    let ir_over_limit = matches!((limit, &ir_latest), (Some(l), Some(m)) if m.ir_mohm.unwrap_or(0) >= l);
    let health_low = matches!((health_limit, retention), (Some(hl), Some(r)) if hl != 0 && r < hl);
    let ir_unchecked = ir_latest.is_some() && limit.is_none();

    Summary {
        retention,
        health_limit,
        health_low,
        ir_limit: limit,
        ir_over_limit,
        ir_unchecked,
        unavailable,
        latest,
        setup: setup
            .map(|s| describe_setup(s, instrument_names))
            .unwrap_or_default(),
        series,
        others,
        newer,
        ir_first,
        ir_latest,
        ir_change,
        chart: chart(battery, measurements),
    }
}

// ── Chart ────────────────────────────────────────────────────────────────

const WIDTH: f64 = 600.0;
const HEIGHT: f64 = 150.0;
const PAD_LEFT: f64 = 8.0;
const PAD_RIGHT: f64 = 8.0;
const PAD_TOP: f64 = 12.0;
const PAD_BOTTOM: f64 = 22.0;

fn round1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

#[derive(Debug, Clone, Serialize)]
pub struct ChartPoint {
    pub x: f64,
    pub comparable: bool,
    pub measured_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cap_y: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cap_label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ir_y: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ir_label: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Chart {
    pub width: f64,
    pub height: f64,
    pub series: Vec<ChartPoint>,
    pub others: Vec<ChartPoint>,
    pub cap_line: String,
    pub ir_line: String,
    pub nominal_y: Option<f64>,
    pub nominal: i64,
    pub cap_high: i64,
    pub cap_low: i64,
    pub ir_high: i64,
    pub ir_low: i64,
    pub first: String,
    pub last: String,
}

/// Coordinates for the capacity (blue) and resistance (red) trend lines,
/// sharing one date x-axis; `None` when there is nothing to draw.
pub fn chart(battery: &Battery, measurements: &[Measurement]) -> Option<Chart> {
    let mut points: Vec<&Measurement> = measurements
        .iter()
        .filter(|m| m.capacity_mah.unwrap_or(0) != 0 || m.ir_mohm.unwrap_or(0) != 0)
        .collect();
    if points.is_empty() {
        return None;
    }
    points.sort_by(|a, b| (&a.measured_at, a.id).cmp(&(&b.measured_at, b.id)));

    let cap_values: Vec<f64> = points
        .iter()
        .filter_map(|m| m.capacity_mah.filter(|&v| v != 0))
        .map(|v| v as f64)
        .collect();
    let nominal = battery.nominal_mah.unwrap_or(0);

    let (cap_low, cap_high) = if !cap_values.is_empty() {
        let mut all = cap_values.clone();
        if nominal != 0 {
            all.push(nominal as f64);
        }
        let mut low = all.iter().cloned().fold(f64::INFINITY, f64::min);
        let mut high = all.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        if high == low {
            low -= 50.0;
            high += 50.0;
        }
        let span = high - low;
        (low - span * 0.08, high + span * 0.08)
    } else {
        ((nominal as f64), (nominal as f64))
    };

    let ir_values: Vec<f64> = points
        .iter()
        .filter_map(|m| m.ir_mohm.filter(|&v| v != 0))
        .map(|v| v as f64)
        .collect();
    let (ir_low, ir_high) = if !ir_values.is_empty() {
        let mut low = ir_values.iter().cloned().fold(f64::INFINITY, f64::min);
        let mut high = ir_values.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        if high == low {
            low = (low - 10.0).max(0.0);
            high += 10.0;
        }
        let span = high - low;
        ((low - span * 0.08).max(0.0), high + span * 0.08)
    } else {
        (0.0, 0.0)
    };

    let dates: Vec<NaiveDate> = points.iter().filter_map(|m| as_date(&m.measured_at)).collect();
    let (first, last) = if !dates.is_empty() {
        (
            dates.iter().copied().min().unwrap(),
            dates.iter().copied().max().unwrap(),
        )
    } else {
        // Sentinel; unused for placement when there are no parseable dates,
        // since `days` stays 0 and x falls back to even spacing.
        let today = chrono::Utc::now().date_naive();
        (today, today)
    };
    let days = (last - first).num_days();
    let have_dates = !dates.is_empty();

    let x_of = |m: &Measurement, index: usize, total: usize| -> f64 {
        let moment = as_date(&m.measured_at);
        match moment {
            Some(moment) if have_dates && days != 0 => {
                let share = (moment - first).num_days() as f64 / days as f64;
                PAD_LEFT + share * (WIDTH - PAD_LEFT - PAD_RIGHT)
            }
            _ => {
                let share = if total == 1 {
                    0.5
                } else {
                    index as f64 / (total - 1) as f64
                };
                PAD_LEFT + share * (WIDTH - PAD_LEFT - PAD_RIGHT)
            }
        }
    };
    let y_of_capacity = |value: f64| -> f64 {
        if cap_values.is_empty() || cap_high == cap_low {
            return HEIGHT / 2.0;
        }
        let share = (value - cap_low) / (cap_high - cap_low);
        HEIGHT - PAD_BOTTOM - share * (HEIGHT - PAD_TOP - PAD_BOTTOM)
    };
    let y_of_resistance = |value: f64| -> f64 {
        if ir_values.is_empty() || ir_high == ir_low {
            return HEIGHT / 2.0;
        }
        let share = (value - ir_low) / (ir_high - ir_low);
        HEIGHT - PAD_BOTTOM - share * (HEIGHT - PAD_TOP - PAD_BOTTOM)
    };

    let total = points.len();
    let drawn: Vec<ChartPoint> = points
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let x = round1(x_of(m, i, total));
            let cap = m.capacity_mah.filter(|&v| v != 0);
            let ir = m.ir_mohm.filter(|&v| v != 0);
            ChartPoint {
                x,
                comparable: true,
                measured_at: m.measured_at.clone(),
                cap_y: cap.map(|v| round1(y_of_capacity(v as f64))),
                cap_label: cap.map(|v| format!("{v} mAh")),
                ir_y: ir.map(|v| round1(y_of_resistance(v as f64))),
                ir_label: ir.map(|v| format!("{v} m\u{03A9}")),
            }
        })
        .collect();

    let cap_line = drawn
        .iter()
        .filter_map(|p| p.cap_y.map(|y| format!("{},{}", p.x, y)))
        .collect::<Vec<_>>()
        .join(" ");
    let ir_line = drawn
        .iter()
        .filter_map(|p| p.ir_y.map(|y| format!("{},{}", p.x, y)))
        .collect::<Vec<_>>()
        .join(" ");

    let nominal_y = if nominal != 0
        && !cap_values.is_empty()
        && (nominal as f64) >= cap_low
        && (nominal as f64) <= cap_high
    {
        Some(round1(y_of_capacity(nominal as f64)))
    } else {
        None
    };

    Some(Chart {
        width: WIDTH,
        height: HEIGHT,
        series: drawn,
        others: Vec::new(),
        cap_line,
        ir_line,
        nominal_y,
        nominal,
        cap_high: if !cap_values.is_empty() {
            round_half_even(cap_high)
        } else {
            0
        },
        cap_low: if !cap_values.is_empty() {
            round_half_even(cap_low)
        } else {
            0
        },
        ir_high: if !ir_values.is_empty() {
            round_half_even(ir_high)
        } else {
            0
        },
        ir_low: if !ir_values.is_empty() {
            round_half_even(ir_low)
        } else {
            0
        },
        first: if have_dates { first.to_string() } else { String::new() },
        last: if have_dates { last.to_string() } else { String::new() },
    })
}
