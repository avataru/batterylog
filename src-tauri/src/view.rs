//! JSON view-model builders: turn `core` domain types into the exact shapes
//! the Tera templates expect (with health/condition fields pre-computed,
//! since Tera's expression language can't run `health::summarise` itself).

use std::collections::HashMap;

use batteries_core::config;
use batteries_core::db::{Battery, Db, Instrument, Measurement, Mode};
use batteries_core::health;
use serde_json::{json, Value};

pub fn instrument_names(db: &Db) -> HashMap<i64, String> {
    db.list_instruments()
        .unwrap_or_default()
        .into_iter()
        .map(|i| (i.id, i.name))
        .collect()
}

pub fn mode_names(db: &Db) -> HashMap<i64, String> {
    let mut out = HashMap::new();
    for i in db.list_instruments().unwrap_or_default() {
        for m in i.modes {
            out.insert(m.id, m.name);
        }
    }
    out
}

pub fn ir_limits(db: &Db) -> HashMap<String, i64> {
    let mut limits = HashMap::new();
    limits.insert("AA".to_string(), config::get(db, "ir_limit_aa").as_i64());
    limits.insert("AAA".to_string(), config::get(db, "ir_limit_aaa").as_i64());
    limits
}

/// A battery + its computed health, shaped for `list.html`'s row/tile
/// macros (`has_health_pct`/`health_pct` split so a genuine 0% doesn't read
/// as "no data" — a plain `{% if b.health_pct %}` in the template would
/// treat 0 as falsy same as missing, so presence is its own explicit
/// boolean instead).
pub fn battery_row(
    db: &Db,
    battery: &Battery,
    limits: &HashMap<String, i64>,
    health_limit: Option<i64>,
    names: &HashMap<i64, String>,
    highest_id: i64,
) -> Value {
    let measurements = db.measurements(battery.id).unwrap_or_default();
    let summary = health::summarise(battery, &measurements, limits, health_limit, names);
    let will_hard_delete = battery.id == highest_id && measurements.is_empty();

    let mut title_parts = Vec::new();
    if let Some(r) = summary.retention {
        title_parts.push(format!("Health: {r}% of nominal"));
    }
    if summary.ir_over_limit {
        if let Some(latest) = &summary.ir_latest {
            title_parts.push(format!(
                "\u{26a0} Resistance: {} m\u{3a9} (over limit)",
                latest.ir_mohm.unwrap_or(0)
            ));
        }
    }

    json!({
        "id": battery.id,
        "type": battery.r#type,
        "nominal_mah": battery.nominal_mah,
        "brand": battery.brand,
        "location": battery.location,
        "deleted_at": battery.deleted_at,
        "has_health_pct": summary.retention.is_some(),
        "health_pct": summary.retention.unwrap_or(0),
        "health_color": health::health_color(summary.retention, health_limit),
        "ir_over_limit": summary.ir_over_limit,
        "health_title": title_parts.join(" \u{b7} "),
        "will_hard_delete": will_hard_delete,
    })
}

/// The detail page's "Condition" card, shaped for `detail.html`.
pub fn condition_view(
    battery: &Battery,
    measurements: &[Measurement],
    limits: &HashMap<String, i64>,
    health_limit: Option<i64>,
    names: &HashMap<i64, String>,
) -> Value {
    let summary = health::summarise(battery, measurements, limits, health_limit, names);
    let ir_latest_instrument_name = summary.ir_latest.as_ref().map(|m| match m.instrument_id {
        Some(id) => names.get(&id).cloned().unwrap_or_else(|| "a deleted instrument".to_string()),
        None => "an unnamed instrument".to_string(),
    });

    json!({
        "has_retention": summary.retention.is_some(),
        "retention": summary.retention.unwrap_or(0),
        "health_limit": health_limit,
        "health_low": summary.health_low,
        "ir_limit": summary.ir_limit,
        "ir_over_limit": summary.ir_over_limit,
        "ir_unchecked": summary.ir_unchecked,
        "unavailable": summary.unavailable,
        "latest": summary.latest,
        "setup": summary.setup,
        "newer": summary.newer,
        "ir_first": summary.ir_first,
        "ir_latest": summary.ir_latest,
        "ir_latest_instrument_name": ir_latest_instrument_name,
        "has_ir_change": summary.ir_change.is_some(),
        "ir_change": summary.ir_change.unwrap_or(0),
        "ir_change_abs": summary.ir_change.map(|c| c.abs()).unwrap_or(0),
        "chart": summary.chart,
    })
}

/// A reading row for the detail page's Readings table, with the instrument/
/// procedure names already resolved.
pub fn reading_row(m: &Measurement, names: &HashMap<i64, String>, modes: &HashMap<i64, String>) -> Value {
    let instrument_name = m.instrument_id.map(|id| {
        names.get(&id).cloned().unwrap_or_else(|| "a deleted instrument".to_string())
    });
    let mode_name = m.mode_id.map(|id| modes.get(&id).cloned().unwrap_or_else(|| "a deleted procedure".to_string()));
    json!({
        "id": m.id,
        "measured_at": m.measured_at,
        "kind": m.kind,
        "capacity_mah": m.capacity_mah,
        "ir_mohm": m.ir_mohm,
        "notes": m.notes,
        "instrument_id": m.instrument_id,
        "mode_id": m.mode_id,
        "discharge_ma": m.discharge_ma,
        "charge_ma": m.charge_ma,
        "instrument_name": instrument_name,
        "mode_name": mode_name,
        "has_protocol": m.mode_id.map(|id| modes.contains_key(&id)).unwrap_or(false),
    })
}

fn field_flag(fields: &Value, key: &str) -> Value {
    let visible = fields.get(key).and_then(|f| f.get("visible")).and_then(|v| v.as_bool()).unwrap_or(true);
    let required = fields.get(key).and_then(|f| f.get("required")).and_then(|v| v.as_bool()).unwrap_or(false);
    json!({ "visible": visible, "required": required })
}

/// A procedure (`instrument_modes` row) shaped for the instruments page's
/// edit form, with its `fields` JSON resolved into a fixed-shape object
/// (`values.fields.capacity_mah.visible`, etc.) instead of a dict-get.
pub fn editable_mode(m: &Mode) -> Value {
    let fields = batteries_core::db::mode_fields(m);
    json!({
        "id": m.id,
        "name": m.name,
        "kind": m.kind,
        "details": m.details,
        "fields": {
            "capacity_mah": field_flag(&fields, "capacity_mah"),
            "ir_mohm": field_flag(&fields, "ir_mohm"),
            "discharge_ma": field_flag(&fields, "discharge_ma"),
            "charge_ma": field_flag(&fields, "charge_ma"),
            "notes": field_flag(&fields, "notes"),
        },
    })
}

pub fn instrument_with_modes(i: &Instrument) -> Value {
    json!({
        "id": i.id,
        "name": i.name,
        "slots": i.slots,
        "modes": i.modes.iter().map(|m| json!({
            "id": m.id, "name": m.name, "kind": m.kind, "fields": m.fields,
        })).collect::<Vec<_>>(),
    })
}
