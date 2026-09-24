//! Suggesting which pooled batteries to hand out for a request.
//!
//! Pure functions only: the caller loads the pool's batteries and
//! measurements and resolves the two config knobs (pool location, the
//! assumed resistance for a cell with no reading), and this module just
//! ranks and picks. Nothing here touches the database.

use std::collections::HashMap;

use crate::db::{Battery, Measurement};
use crate::health;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Draw {
    /// A low-current device: a weak cell is still safe to use, so it's
    /// used up first rather than left sitting in the pool.
    Low,
    /// A high-current device: a weak cell can sag or fail under load, so
    /// the strongest cells are picked instead.
    High,
}

impl Draw {
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "low" => Some(Draw::Low),
            "high" => Some(Draw::High),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Draw::Low => "low",
            Draw::High => "high",
        }
    }
}

/// A pool battery with the numbers the matcher ranks it on already pulled
/// out of its measurement history.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub battery: Battery,
    pub capacity_mah: i64,
    pub ir_mohm: i64,
    /// `true` when `ir_mohm` isn't a real reading, just the configured
    /// default for a cell that has a capacity reading but no resistance
    /// one yet.
    pub ir_assumed: bool,
}

/// The pool: active batteries sitting in `pool_location` (compared
/// case-insensitively) that have at least one capacity reading. A battery
/// with no capacity reading at all is left out entirely rather than
/// ranked — it needs to be measured first, not guessed at.
pub fn eligible_pool(
    batteries: &[Battery],
    measurements_by_battery: &HashMap<i64, Vec<Measurement>>,
    pool_location: &str,
    default_ir_mohm: i64,
) -> Vec<Candidate> {
    let empty: Vec<Measurement> = Vec::new();
    batteries
        .iter()
        .filter(|b| b.deleted_at.is_none() && b.location.eq_ignore_ascii_case(pool_location))
        .filter_map(|b| {
            let measurements = measurements_by_battery.get(&b.id).unwrap_or(&empty);
            let capacity_mah = health::analyses(measurements).last()?.capacity_mah?;
            let ir_reading = measurements
                .iter()
                .filter(|m| m.ir_mohm.unwrap_or(0) != 0)
                .max_by(|a, c| (&a.measured_at, a.id).cmp(&(&c.measured_at, c.id)));
            let (ir_mohm, ir_assumed) = match ir_reading.and_then(|m| m.ir_mohm) {
                Some(ir) => (ir, false),
                None => (default_ir_mohm, true),
            };
            Some(Candidate { battery: b.clone(), capacity_mah, ir_mohm, ir_assumed })
        })
        .collect()
}

/// Higher is better. Capacity retention (against the cell's own nominal
/// capacity, so cells of different nominal capacities stay comparable)
/// dominates; resistance nudges the ranking, weighted so it only ever
/// breaks a close call rather than overriding a real retention gap.
fn strength_score(c: &Candidate) -> f64 {
    let retention = match c.battery.nominal_mah {
        Some(nominal) if nominal != 0 => c.capacity_mah as f64 / nominal as f64 * 100.0,
        _ => c.capacity_mah as f64,
    };
    retention - (c.ir_mohm as f64 / 20.0)
}

fn rank<'a>(mut group: Vec<&'a Candidate>, draw: Draw) -> Vec<&'a Candidate> {
    group.sort_by(|a, b| {
        let ord = strength_score(a)
            .partial_cmp(&strength_score(b))
            .unwrap_or(std::cmp::Ordering::Equal);
        match draw {
            Draw::Low => ord,
            Draw::High => ord.reverse(),
        }
    });
    group
}

#[derive(Debug, Clone)]
pub struct Suggestion {
    pub battery_ids: Vec<i64>,
    /// `true` when no single (brand, nominal capacity) bucket in the pool
    /// had enough cells, so the pick mixes brands and/or capacities.
    pub mixed: bool,
    pub ir_assumed_ids: Vec<i64>,
}

/// Picks `count` batteries of `requested_type` out of `candidates`.
///
/// Prefers a single (brand, nominal capacity) bucket big enough to fill
/// the request on its own; only mixes across brands/capacities when no
/// one bucket has enough. When several buckets are each big enough, the
/// one whose picked cells best match `draw` (weakest for low-draw,
/// strongest for high-draw) wins. Within whatever's chosen, cells are
/// ranked by [`strength_score`] and the top `count` are picked, then
/// returned in id order rather than rank order.
pub fn suggest(
    candidates: &[Candidate],
    requested_type: &str,
    count: usize,
    draw: Draw,
) -> Result<Suggestion, String> {
    if count == 0 {
        return Err("Ask for at least one battery.".to_string());
    }

    let type_matches: Vec<&Candidate> = candidates
        .iter()
        .filter(|c| c.battery.r#type.eq_ignore_ascii_case(requested_type))
        .collect();

    let mut buckets: HashMap<(String, Option<i64>), Vec<&Candidate>> = HashMap::new();
    for &c in &type_matches {
        buckets
            .entry((c.battery.brand.clone(), c.battery.nominal_mah))
            .or_default()
            .push(c);
    }

    let mut best: Option<(Vec<&Candidate>, f64)> = None;
    for group in buckets.into_values() {
        if group.len() < count {
            continue;
        }
        let ranked = rank(group, draw);
        let avg: f64 = ranked[..count].iter().map(|c| strength_score(c)).sum::<f64>() / count as f64;
        let better = match &best {
            None => true,
            Some((_, best_avg)) => match draw {
                Draw::Low => avg < *best_avg,
                Draw::High => avg > *best_avg,
            },
        };
        if better {
            best = Some((ranked, avg));
        }
    }

    let (ranked, mixed) = match best {
        Some((ranked, _)) => (ranked, false),
        None if type_matches.len() >= count => (rank(type_matches, draw), true),
        None => {
            return Err(format!(
                "Only {} \"{requested_type}\" batter{} available in storage; {count} requested.",
                type_matches.len(),
                if type_matches.len() == 1 { "y is" } else { "ies are" },
            ));
        }
    };

    let picked = &ranked[..count];
    let mut battery_ids: Vec<i64> = picked.iter().map(|c| c.battery.id).collect();
    battery_ids.sort_unstable();
    Ok(Suggestion {
        battery_ids,
        mixed,
        ir_assumed_ids: picked.iter().filter(|c| c.ir_assumed).map(|c| c.battery.id).collect(),
    })
}
