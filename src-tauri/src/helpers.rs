//! Small parsing/validation helpers shared across routes.

use std::collections::HashMap;

use batteries_core::db::Db;

/// `"7"`, `"5,7,9"`, `"5-9"`, or any mixture, e.g. `"1, 3, 5-8, 12"`. Ranges
/// are capped at 200 ids each, so a typo like `"1-999999"` can't try to
/// resolve a huge range.
pub fn parse_id_spec(raw: &str) -> Result<Vec<i64>, String> {
    let mut ids = Vec::new();
    for part in raw.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some((a, b)) = part.split_once('-') {
            let a: i64 = a.trim().parse().map_err(|_| format!("{part:?} is not a valid id or range."))?;
            let b: i64 = b.trim().parse().map_err(|_| format!("{part:?} is not a valid id or range."))?;
            let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
            if hi - lo + 1 > 200 {
                return Err(format!("{part:?} spans more than 200 ids."));
            }
            for id in lo..=hi {
                ids.push(id);
            }
        } else {
            let id: i64 = part.parse().map_err(|_| format!("{part:?} is not a valid id or range."))?;
            ids.push(id);
        }
    }
    Ok(ids)
}

pub fn parse_positive_int(raw: &str) -> Option<i64> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    raw.parse::<i64>().ok().filter(|&n| n > 0)
}

pub fn whole_number(raw: &str) -> Option<i64> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    raw.parse::<i64>().ok()
}

/// Like `whole_number`, but a value that parses fine except for being
/// negative (or isn't a number at all) is a validation error rather than a
/// silent `None` — capacity/resistance/discharge/charge readings are never
/// negative, so a `-5` is a typo worth surfacing, not a value to discard.
pub fn unsigned_number(raw: &str, label: &str) -> Result<Option<i64>, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(None);
    }
    match raw.parse::<i64>() {
        Ok(n) if n >= 0 => Ok(Some(n)),
        Ok(_) => Err(format!("{label} cannot be negative.")),
        Err(_) => Err(format!("{label} must be a whole number.")),
    }
}

/// The ±2-around-current-page pagination window, with `0` used as a "…" gap
/// marker for the template to render as an ellipsis.
pub fn page_numbers(page: i64, pages: i64) -> Vec<i64> {
    if pages <= 1 {
        return Vec::new();
    }
    let mut shown: Vec<i64> = Vec::new();
    shown.push(1);
    for n in (page - 2)..=(page + 2) {
        if n > 1 && n < pages {
            shown.push(n);
        }
    }
    shown.push(pages);
    shown.dedup();
    shown.sort_unstable();

    let mut out = Vec::new();
    let mut prev: Option<i64> = None;
    for n in shown {
        if let Some(p) = prev {
            if n - p > 1 {
                out.push(0);
            }
        }
        out.push(n);
        prev = Some(n);
    }
    out
}

/// A redirect target must be a same-site absolute path, never `//host/...`
/// (which browsers treat as protocol-relative to another origin).
pub fn safe_path(raw: &str, fallback: &str) -> String {
    if raw.starts_with('/') && !raw.starts_with("//") {
        raw.to_string()
    } else {
        fallback.to_string()
    }
}

/// The single validation routine shared by the detail-page measure form, the
/// scan-dialog API, and the batch-measure form.
#[allow(clippy::too_many_arguments)]
pub fn clean_reading(
    db: &Db,
    kind: &str,
    capacity_mah: Option<i64>,
    ir_mohm: Option<i64>,
    instrument_id: Option<i64>,
    mode_id: Option<i64>,
    discharge_ma: Option<i64>,
    charge_ma: Option<i64>,
) -> Result<(), String> {
    if kind == "bought" {
        return Ok(());
    }
    if capacity_mah.is_some() && discharge_ma.is_none() {
        return Err("A capacity needs the discharge current it was measured at.".to_string());
    }
    let has_any = capacity_mah.is_some()
        || ir_mohm.is_some()
        || discharge_ma.is_some()
        || charge_ma.is_some();
    if has_any && instrument_id.is_none() {
        return Err("Any reading needs the instrument it came from.".to_string());
    }
    if !has_any {
        return Err(format!("A {kind} reading needs at least one value."));
    }
    if let Some(id) = instrument_id {
        if db.instrument(id).map_err(|e| e.to_string())?.is_none() {
            return Err(format!("No instrument has id {id}."));
        }
    }
    if let Some(id) = mode_id {
        if db.mode(id).map_err(|e| e.to_string())?.is_none() {
            return Err(format!("No procedure has id {id}."));
        }
    }
    Ok(())
}

pub fn get_str<'a>(fields: &'a HashMap<String, String>, key: &str) -> &'a str {
    fields.get(key).map(|s| s.as_str()).unwrap_or("")
}
