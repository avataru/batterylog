//! Every route, dispatched as two functions called over Tauri's IPC instead
//! of HTTP: `render_page` for GET-shaped navigation, `submit_form` for
//! POST-shaped form actions. Each returns either rendered HTML for the
//! resulting page, or a redirect the shell resolves by calling
//! `render_page` again — a post/redirect/get shape without an actual
//! network round trip.

use std::collections::{BTreeMap, HashMap};

use batteries_core::db::{Battery, Db};
use batteries_core::{config, health, labels, matching};
use serde::Serialize;
use serde_json::{json, Value};

use crate::helpers::*;
use crate::state::AppState;
use crate::view;

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CommandResult {
    Page {
        html: String,
        title: String,
        open_dialog: Option<String>,
    },
    Redirect {
        path: String,
        query: Option<String>,
    },
}

/// `path` may be a bare path (`"/instruments"`) or a full href that already
/// carries its own query (`next`/`list_url` values do, by design, so a
/// filtered list can be returned to after an action) — either way, `path`
/// ends up holding just the route and `query` holds everything merged, so
/// the JS shell's `render_page` call always gets them as the two separate
/// fields the router matches on. A `?` left inside `path` here previously
/// caused a hard 404 (path segments are split on `/`, not `?`, so
/// `/instruments?open=6` matched nothing) and, more recently, an actual
/// panic from an over-eager assert that didn't account for `next` carrying
/// its own query — this split-and-merge is the real fix for both.
fn redirect(path: impl Into<String>, pairs: &[(&str, &str)]) -> CommandResult {
    let raw = path.into();
    let (bare_path, embedded_query) = match raw.split_once('?') {
        Some((p, q)) => (p.to_string(), Some(q.to_string())),
        None => (raw, None),
    };
    let extra_query = query_string(pairs);
    let query = match (embedded_query, extra_query.is_empty()) {
        (Some(q), true) => Some(q),
        (Some(q), false) => Some(format!("{q}&{extra_query}")),
        (None, true) => None,
        (None, false) => Some(extra_query),
    };
    CommandResult::Redirect { path: bare_path, query }
}

fn query_string(pairs: &[(&str, &str)]) -> String {
    let mut s = url::form_urlencoded::Serializer::new(String::new());
    for (k, v) in pairs {
        if !v.is_empty() {
            s.append_pair(k, v);
        }
    }
    s.finish()
}

pub fn parse_query(query: &str) -> HashMap<String, String> {
    url::form_urlencoded::parse(query.as_bytes()).into_owned().collect()
}

fn page(
    state: &AppState,
    template: &str,
    mut ctx: Value,
    title: &str,
    msg: &str,
    error: &str,
    open_dialog: &str,
) -> CommandResult {
    if let Value::Object(map) = &mut ctx {
        map.insert("msg".into(), json!(msg));
        map.insert("error".into(), json!(error));
        map.insert("open_dialog".into(), json!(open_dialog));
    }
    let context = tera::Context::from_value(ctx).expect("context is always an object");
    let html = state.tera.render(template, &context).unwrap_or_else(|e| {
        use std::error::Error;
        let mut chain = e.to_string();
        let mut source = e.source();
        while let Some(s) = source {
            chain.push_str(" -> ");
            chain.push_str(&s.to_string());
            source = s.source();
        }
        format!("<pre>Template error rendering {template}: {chain}</pre>")
    });
    CommandResult::Page {
        html,
        title: title.to_string(),
        open_dialog: if open_dialog.is_empty() { None } else { Some(open_dialog.to_string()) },
    }
}

fn id_list(ids: &[i64]) -> String {
    ids.iter().map(|i| format!("{i:03}")).collect::<Vec<_>>().join(", ")
}

fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

/// `tape_mm` is a `Choice` setting (stored/compared as text, e.g. `"3.5"`),
/// not a `Float`, so it has to be parsed rather than read via `as_f64()`.
fn tape_mm_x10(db: &Db) -> i64 {
    let text = config::get(db, "tape_mm").as_text();
    let mm: f64 = text.parse().unwrap_or(9.0);
    (mm * 10.0).round() as i64
}

/// When true, every "print" action redirects with the relevant ids in a
/// `save_pdf_ids` query field instead of calling `labels::print_labels` —
/// `app.js` picks that field up once the redirect lands and calls
/// `invoke('save_labels_pdf', ...)` directly, since opening a save dialog
/// needs a real Tauri command (an `AppHandle`), not a plain router function.
fn label_output_is_pdf(db: &Db) -> bool {
    config::get(db, "label_output").as_text() == "pdf"
}

// ── GET-shaped navigation ───────────────────────────────────────────────

pub fn render_page(state: &AppState, path: &str, query: &str) -> CommandResult {
    let q = parse_query(query);
    let segments: Vec<&str> = path.trim_matches('/').split('/').filter(|s| !s.is_empty()).collect();
    let msg = q.get("msg").cloned().unwrap_or_default();
    let error = q.get("error").cloned().unwrap_or_default();

    match segments.as_slice() {
        [] => page_list(state, &q, &msg, &error),
        ["add"] => page_add(state, &msg, &error, None),
        ["scan"] => page_scan(state, &msg, &error),
        ["print"] => page_print(state, &msg, &error, None),
        ["instruments"] => page_instruments(state, &q, &msg, &error, None),
        ["settings"] => page_settings(state, &msg, &error, None),
        ["batch", "measure"] => page_batch_measure(state, &msg, &error, None),
        ["batch", "location"] => page_batch_location(state, &q, &msg, &error, None),
        ["match"] => page_match(state, &msg, &error, None),
        ["match", "log"] => page_match_log(state, &msg, &error),
        ["b", id] => {
            if let Ok(id) = id.parse::<i64>() {
                page_detail(state, id, &q, &msg, &error)
            } else {
                not_found_result(state, path)
            }
        }
        ["label", file] if file.ends_with(".png") => not_found_result(state, path),
        _ => not_found_result(state, path),
    }
}

fn not_found_result(state: &AppState, attempted_path: &str) -> CommandResult {
    // `attempted_path` is shown so a routing bug (a redirect target that
    // didn't match any route) is diagnosable from what's on screen, instead
    // of every miss looking identical.
    page(
        state,
        "missing.html",
        json!({"battery_id": format!("? ({attempted_path:?})")}),
        "Not found",
        "",
        "",
        "",
    )
}

/// Show/type/location/search/sort are remembered across navigation (and
/// restarts) in the `settings` table under this key, as a small JSON blob —
/// a plain `db.get_setting`/`set_setting` pair rather than a registered
/// `config::SETTINGS` entry, since this is remembered view state a user
/// changes by using the filters, not a setting they'd browse on the
/// Settings page.
const LIST_FILTERS_KEY: &str = "list_filters";

fn page_list(state: &AppState, q: &HashMap<String, String>, msg: &str, error: &str) -> CommandResult {
    let db = state.db.lock().unwrap();
    let saved = db
        .get_setting(LIST_FILTERS_KEY)
        .ok()
        .flatten()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .unwrap_or_else(|| json!({}));
    let saved_str = |key: &str| -> String {
        saved.get(key).and_then(|v| v.as_str()).unwrap_or("").to_string()
    };

    let show_owned = q.get("show").cloned().unwrap_or_else(|| saved_str("show"));
    let show_owned = if show_owned.is_empty() { "active".to_string() } else { show_owned };
    let show: &str = if ["active", "deleted", "all", "flagged"].contains(&show_owned.as_str()) { &show_owned } else { "active" };
    let type_filter = q.get("type").cloned().unwrap_or_else(|| saved_str("type"));
    let location_filter = q.get("location").cloned().unwrap_or_else(|| saved_str("location"));
    let search = q.get("q").cloned().unwrap_or_else(|| saved_str("q"));
    let sort_owned = q.get("sort").cloned().unwrap_or_else(|| saved_str("sort"));
    let sort_owned = if sort_owned.is_empty() { "id".to_string() } else { sort_owned };
    let sort: &str = if ["id", "type", "location", "health", "resistance"].contains(&sort_owned.as_str()) { &sort_owned } else { "id" };
    let page_num = q.get("page").and_then(|s| s.parse::<i64>().ok()).unwrap_or(1).max(1);

    let _ = db.set_setting(
        LIST_FILTERS_KEY,
        &json!({"show": show, "type": type_filter, "location": location_filter, "q": search, "sort": sort}).to_string(),
    );

    let page_size = config::get(&db, "page_size").as_i64().max(1);
    let grid_columns = config::get(&db, "grid_columns").as_i64();
    let view_mode = q.get("view").cloned().unwrap_or_else(|| config::get(&db, "list_view").as_text());
    let health_limit = Some(config::get(&db, "health_limit_pct").as_i64());
    let limits = view::ir_limits(&db);
    let names = view::instrument_names(&db);

    let db_show = if show == "flagged" { "active" } else { show };
    let batteries = db
        .list_batteries(&type_filter, &location_filter, &search, db_show, "id", None, 0)
        .unwrap_or_default();
    let highest_id = db.next_id().unwrap_or(1) - 1;

    let mut rows: Vec<(Value, Option<i64>)> = batteries
        .iter()
        .map(|b| {
            let measurements = db.measurements(b.id).unwrap_or_default();
            let summary = health::summarise(b, &measurements, &limits, health_limit, &names);
            let ir_mohm = summary.ir_latest.as_ref().and_then(|m| m.ir_mohm);
            (view::battery_row(&db, b, &limits, health_limit, &names, highest_id), ir_mohm)
        })
        .collect();

    if show == "flagged" {
        rows.retain(|(row, ir)| {
            let low_health = row["has_health_pct"].as_bool().unwrap_or(false)
                && row["health_pct"].as_i64().unwrap_or(0) < health_limit.unwrap_or(0);
            let ir_over = row["ir_over_limit"].as_bool().unwrap_or(false);
            low_health || ir.is_some() && ir_over
        });
    }

    match sort {
        "type" => rows.sort_by(|a, b| {
            a.0["type"].as_str().cmp(&b.0["type"].as_str()).then(a.0["id"].as_i64().cmp(&b.0["id"].as_i64()))
        }),
        "location" => rows.sort_by(|a, b| {
            a.0["location"].as_str().cmp(&b.0["location"].as_str()).then(a.0["id"].as_i64().cmp(&b.0["id"].as_i64()))
        }),
        "health" => rows.sort_by(|a, b| {
            let ha = if a.0["has_health_pct"].as_bool().unwrap_or(false) { a.0["health_pct"].as_i64() } else { None };
            let hb = if b.0["has_health_pct"].as_bool().unwrap_or(false) { b.0["health_pct"].as_i64() } else { None };
            // worst (lowest %) first; no data sorts last
            match (ha, hb) {
                (Some(x), Some(y)) => x.cmp(&y),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => a.0["id"].as_i64().cmp(&b.0["id"].as_i64()),
            }
        }),
        "resistance" => rows.sort_by(|a, b| match (a.1, b.1) {
            (Some(x), Some(y)) => y.cmp(&x), // highest first
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => a.0["id"].as_i64().cmp(&b.0["id"].as_i64()),
        }),
        _ => rows.sort_by(|a, b| a.0["id"].as_i64().cmp(&b.0["id"].as_i64())),
    }

    // Taken here because the paging below consumes `rows`.
    let mut type_counts: BTreeMap<String, i64> = BTreeMap::new();
    for (row, _) in &rows {
        *type_counts.entry(row["type"].as_str().unwrap_or("").to_string()).or_insert(0) += 1;
    }

    let total = rows.len() as i64;
    let pages = ((total - 1) / page_size + 1).max(1);
    let page_num = page_num.min(pages);
    let start = ((page_num - 1) * page_size) as usize;
    let page_rows: Vec<Value> = rows.into_iter().skip(start).take(page_size as usize).map(|(r, _)| r).collect();
    let first_row = if total == 0 { 0 } else { start as i64 + 1 };
    let last_row = (start as i64 + page_rows.len() as i64).min(total);

    let types = db.distinct("type").unwrap_or_default();
    let locations = db.distinct("location").unwrap_or_default();
    let brands = db.distinct("brand").unwrap_or_default();

    let filter_query = {
        let mut pairs = Vec::new();
        if !type_filter.is_empty() { pairs.push(("type", type_filter.as_str())); }
        if !location_filter.is_empty() { pairs.push(("location", location_filter.as_str())); }
        if !search.is_empty() { pairs.push(("q", search.as_str())); }
        let s = query_string(&pairs);
        if s.is_empty() { String::new() } else { format!("{s}&") }
    };
    let page_query = {
        let mut pairs = vec![("show", show)];
        if !type_filter.is_empty() { pairs.push(("type", &type_filter)); }
        if !location_filter.is_empty() { pairs.push(("location", &location_filter)); }
        if !search.is_empty() { pairs.push(("q", &search)); }
        if sort != "id" { pairs.push(("sort", sort)); }
        let s = query_string(&pairs);
        if s.is_empty() { String::new() } else { format!("{s}&") }
    };
    let list_url = {
        let mut pairs = vec![("show", show)];
        if !type_filter.is_empty() { pairs.push(("type", &type_filter)); }
        if !location_filter.is_empty() { pairs.push(("location", &location_filter)); }
        if !search.is_empty() { pairs.push(("q", &search)); }
        if sort != "id" { pairs.push(("sort", sort)); }
        if page_num != 1 { pairs.push(("page", Box::leak(page_num.to_string().into_boxed_str()))); }
        let s = query_string(&pairs);
        if s.is_empty() { "/".to_string() } else { format!("/?{s}") }
    };

    let is_flagged = |b: &Battery| -> bool {
        let measurements = db.measurements(b.id).unwrap_or_default();
        let summary = health::summarise(b, &measurements, &limits, health_limit, &names);
        let low_health = summary.retention.map(|r| health_limit.map(|l| r < l).unwrap_or(false)).unwrap_or(false);
        low_health || summary.ir_over_limit
    };

    let deleted_count = db.deleted_count().unwrap_or(0);
    let flagged_count = if show == "flagged" {
        total
    } else {
        let all_active = db.list_batteries("", "", "", "active", "id", None, 0).unwrap_or_default();
        all_active.iter().filter(|b| is_flagged(b)).count() as i64
    };

    let heading = if !type_filter.is_empty() {
        // How this type stands across every status, whichever scope is
        // being listed — "3x active, 2x flagged, 2x deleted".
        let of_type = |scope: &str| {
            db.list_batteries(&type_filter, &location_filter, &search, scope, "id", None, 0).unwrap_or_default()
        };
        let active = of_type("active");
        let flagged = active.iter().filter(|b| is_flagged(b)).count();
        let deleted = of_type("deleted").len();
        let mut parts = vec![format!("{}x active", active.len())];
        if flagged > 0 {
            parts.push(format!("{flagged}x flagged"));
        }
        if deleted > 0 {
            parts.push(format!("{deleted}x deleted"));
        }
        parts.join(", ")
    } else {
        match show {
            "deleted" => format!("{total} deleted batter{}.", if total == 1 { "y" } else { "ies" }),
            "flagged" => format!("{total} flagged batter{}.", if total == 1 { "y" } else { "ies" }),
            "all" => format!("{total} batter{} total.", if total == 1 { "y" } else { "ies" }),
            _ if type_counts.is_empty() => "No batteries.".to_string(),
            _ => type_counts.iter().map(|(t, n)| format!("{n}x {t}")).collect::<Vec<_>>().join(", "),
        }
    };

    // Nothing to distinguish "active" from "all" when nothing is deleted or
    // flagged, so the whole scope-picker collapses to nothing worth showing.
    let has_variety = deleted_count > 0 || flagged_count > 0;
    let mut scopes = Vec::new();
    if has_variety {
        scopes.push(json!({"value": "active", "text": "Active"}));
    }
    if flagged_count > 0 {
        scopes.push(json!({"value": "flagged", "text": "Flagged"}));
    }
    if deleted_count > 0 {
        scopes.push(json!({"value": "deleted", "text": "Deleted"}));
    }
    if has_variety {
        scopes.push(json!({"value": "all", "text": "All"}));
    }

    let ctx = json!({
        "heading": heading,
        "scopes": scopes,
        "show": show,
        "filter_query": filter_query,
        "view": view_mode,
        "list_url": list_url,
        "type_filter": type_filter,
        "location_filter": location_filter,
        "search": search,
        "sort": sort,
        "types": types,
        "locations": locations,
        "brands": brands,
        "batteries": page_rows,
        "total": total,
        "grid_columns": grid_columns,
        "pages": pages,
        "page": page_num,
        "first_row": first_row,
        "last_row": last_row,
        "page_query": page_query,
        "page_numbers": page_numbers(page_num, pages),
    });
    page(state, "list.html", ctx, "Batteries", msg, error, "")
}

fn page_add(state: &AppState, msg: &str, error: &str, extra: Option<Value>) -> CommandResult {
    let db = state.db.lock().unwrap();
    let printer = labels::printer_status(&db);
    let suggested_id = db.next_id().unwrap_or(1);
    let types = db.distinct("type").unwrap_or_default();
    let brands = db.distinct("brand").unwrap_or_default();
    let locations = db.distinct("location").unwrap_or_default();
    let top_locations = db.popular("location", 8).unwrap_or_default();
    let print_on_add = config::get(&db, "print_on_add").as_bool();

    let mut ctx = json!({
        "printer": {"available": printer.available, "description": printer.description},
        "suggested_id": suggested_id,
        "types": types, "brands": brands, "locations": locations,
        "top_locations": top_locations,
        "print_on_add": print_on_add,
        "add_error": "",
        "add_values": {},
    });
    if let (Some(extra), Value::Object(map)) = (extra, &mut ctx) {
        if let Value::Object(extra_map) = extra {
            for (k, v) in extra_map {
                map.insert(k, v);
            }
        }
    }
    page(state, "add.html", ctx, "Add battery", msg, error, "")
}

fn page_scan(state: &AppState, msg: &str, error: &str) -> CommandResult {
    let db = state.db.lock().unwrap();
    let locations = db.distinct("location").unwrap_or_default();
    let top_locations = db.popular("location", 8).unwrap_or_default();
    let instruments: Vec<Value> = db.list_instruments().unwrap_or_default().iter().filter(|i| !i.modes.is_empty()).map(view::instrument_with_modes).collect();
    let (instrument_id, mode_id, discharge_ma, charge_ma) = db.last_reading_defaults().unwrap_or((None, None, None, None));

    let ctx = json!({
        "kinds": batteries_core::db::KINDS,
        "today": today(),
        "instruments": instruments,
        "last_used": {
            "instrument_id": instrument_id, "mode_id": mode_id,
            "discharge_ma": discharge_ma, "charge_ma": charge_ma,
        },
        "locations": locations, "top_locations": top_locations,
    });
    page(state, "scan.html", ctx, "Scan", msg, error, "")
}

fn page_print(state: &AppState, msg: &str, error: &str, extra: Option<Value>) -> CommandResult {
    let mut ctx = json!({"print_error": "", "print_spec": ""});
    if let (Some(extra), Value::Object(map)) = (extra, &mut ctx) {
        if let Value::Object(extra_map) = extra {
            for (k, v) in extra_map { map.insert(k, v); }
        }
    }
    page(state, "print.html", ctx, "Print labels", msg, error, "")
}

fn page_instruments(state: &AppState, _q: &HashMap<String, String>, msg: &str, error: &str, extra: Option<Value>) -> CommandResult {
    let db = state.db.lock().unwrap();
    let instruments: Vec<Value> = db
        .list_instruments()
        .unwrap_or_default()
        .iter()
        .map(|i| {
            json!({
                "id": i.id, "name": i.name, "slots": i.slots,
                "modes": i.modes.iter().map(view::editable_mode).collect::<Vec<_>>(),
            })
        })
        .collect();
    let mut ctx = json!({
        "instruments": instruments,
        "procedure_kinds": batteries_core::db::PROCEDURE_KINDS,
        "add_instrument_open": false, "add_instrument_error": "", "add_instrument_values": {},
        "edit_instrument_for": Value::Null, "edit_instrument_error": "", "edit_instrument_values": {},
        "edit_mode_for": Value::Null, "edit_mode_error": "", "edit_mode_values": {},
        "add_mode_for": Value::Null, "add_mode_error": "", "add_mode_values": {},
        "empty_mode_values": {"name": "", "kind": "analysed", "details": ""},
    });
    if let (Some(extra), Value::Object(map)) = (extra, &mut ctx) {
        if let Value::Object(extra_map) = extra {
            for (k, v) in extra_map { map.insert(k, v); }
        }
    }
    page(state, "instruments.html", ctx, "Instruments", msg, error, "")
}

fn page_settings(state: &AppState, msg: &str, error: &str, extra: Option<Value>) -> CommandResult {
    let db = state.db.lock().unwrap();
    let sections = config::overview(&db);
    let mut ctx = json!({"settings": sections});
    if let (Some(extra), Value::Object(map)) = (extra, &mut ctx) {
        if let Value::Object(extra_map) = extra { for (k, v) in extra_map { map.insert(k, v); } }
    }
    page(state, "settings.html", ctx, "Settings", msg, error, "")
}

fn page_batch_measure(state: &AppState, msg: &str, error: &str, extra: Option<Value>) -> CommandResult {
    let db = state.db.lock().unwrap();
    let instruments: Vec<Value> = db.list_instruments().unwrap_or_default().iter().filter(|i| !i.modes.is_empty()).map(view::instrument_with_modes).collect();

    let mut ctx = json!({
        "kinds": batteries_core::db::KINDS,
        "today": today(),
        "instruments": instruments,
        "common": {},
        "slot_rows": (1..=16i64).map(|i| json!({"i": i, "battery_id": "", "capacity_mah": "", "ir_mohm": "", "discharge_ma": "", "charge_ma": "", "notes": ""})).collect::<Vec<_>>(),
        "bought_rows": (1..=10i64).map(|i| json!({"i": i, "battery_id": ""})).collect::<Vec<_>>(),
        "page_error": "",
    });
    if let (Some(extra), Value::Object(map)) = (extra, &mut ctx) {
        if let Value::Object(extra_map) = extra { for (k, v) in extra_map { map.insert(k, v); } }
    }
    page(state, "batch_measure.html", ctx, "Batch add reading", msg, error, "")
}

fn page_batch_location(state: &AppState, _q: &HashMap<String, String>, msg: &str, error: &str, extra: Option<Value>) -> CommandResult {
    let db = state.db.lock().unwrap();
    let locations = db.distinct("location").unwrap_or_default();
    let top_locations = db.popular("location", 8).unwrap_or_default();
    let mut ctx = json!({
        "locations": locations, "top_locations": top_locations,
        "spec": "", "location": "", "page_error": "",
    });
    if let (Some(extra), Value::Object(map)) = (extra, &mut ctx) {
        if let Value::Object(extra_map) = extra { for (k, v) in extra_map { map.insert(k, v); } }
    }
    page(state, "batch_location.html", ctx, "Batch change location", msg, error, "")
}

fn page_match(state: &AppState, msg: &str, error: &str, extra: Option<Value>) -> CommandResult {
    let db = state.db.lock().unwrap();
    let types = db.distinct("type").unwrap_or_default();
    let locations = db.distinct("location").unwrap_or_default();
    let top_locations = db.popular("location", 8).unwrap_or_default();
    let pool_location = config::get(&db, "pool_location").as_text();
    drop(db);

    let mut ctx = json!({
        "types": types, "locations": locations, "top_locations": top_locations,
        "pool_location": pool_location,
        "match_type": "", "match_count": "", "match_draw": "low", "match_location": "",
        "page_error": "",
        "suggestion": Value::Null,
    });
    if let (Some(extra), Value::Object(map)) = (extra, &mut ctx) {
        if let Value::Object(extra_map) = extra { for (k, v) in extra_map { map.insert(k, v); } }
    }
    page(state, "match.html", ctx, "Match batteries", msg, error, "")
}

fn page_match_log(state: &AppState, msg: &str, error: &str) -> CommandResult {
    let db = state.db.lock().unwrap();
    let matches = db.list_matches().unwrap_or_default();
    let pool_location = config::get(&db, "pool_location").as_text();
    drop(db);
    let ctx = json!({ "matches": matches, "pool_location": pool_location });
    page(state, "match_log.html", ctx, "Match log", msg, error, "")
}

fn page_detail(state: &AppState, id: i64, q: &HashMap<String, String>, msg: &str, error: &str) -> CommandResult {
    let db = state.db.lock().unwrap();
    let battery = match db.get(id).unwrap_or(None) {
        Some(b) => b,
        None => return page(state, "missing.html", json!({"battery_id": id}), &format!("Battery {id}"), "", "", ""),
    };

    let tab = q.get("tab").cloned().unwrap_or_default();
    let dialog = q.get("dialog").cloned().unwrap_or_default();
    let (prev, next) = db.adjacent_ids(id).unwrap_or((None, None));
    let measurements = db.measurements(id).unwrap_or_default();
    let history = db.history(id).unwrap_or_default();
    let limits = view::ir_limits(&db);
    let health_limit = Some(config::get(&db, "health_limit_pct").as_i64());
    let names = view::instrument_names(&db);
    let modes = view::mode_names(&db);
    let condition = view::condition_view(&battery, &measurements, &limits, health_limit, &names);
    let readings: Vec<Value> = measurements.iter().map(|m| view::reading_row(m, &names, &modes)).collect();

    let instruments: Vec<Value> = db.list_instruments().unwrap_or_default().iter().filter(|i| !i.modes.is_empty()).map(view::instrument_with_modes).collect();
    let types = db.distinct("type").unwrap_or_default();
    let brands = db.distinct("brand").unwrap_or_default();
    let locations = db.distinct("location").unwrap_or_default();
    let top_locations = db.popular("location", 8).unwrap_or_default();
    let (last_instrument, last_mode, last_discharge, last_charge) = db.last_reading_defaults().unwrap_or((None, None, None, None));
    let highest_id = db.next_id().unwrap_or(1) - 1;
    let will_hard_delete = battery.id == highest_id && measurements.is_empty();
    let can_purge = battery.deleted_at.is_some() && measurements.is_empty();

    let ctx = json!({
        "battery": battery,
        "will_hard_delete": will_hard_delete,
        "can_purge": can_purge,
        "adjacent": {"prev": prev, "next": next},
        "tab": tab,
        "condition": condition,
        "history": history,
        "readings": readings,
        "instruments": instruments,
        "kinds": batteries_core::db::KINDS,
        "today": today(),
        "types": types, "brands": brands, "locations": locations, "top_locations": top_locations,
        "measure_error": "",
        "measure": {
            "instrument_id": last_instrument, "mode_id": last_mode,
            "discharge_ma": last_discharge, "charge_ma": last_charge,
        },
    });
    let title = format!("Battery {id:03}");
    page(state, "detail.html", ctx, &title, msg, error, &dialog)
}

// ── POST-shaped form actions ────────────────────────────────────────────

pub fn submit_form(state: &AppState, path: &str, fields: &HashMap<String, String>) -> CommandResult {
    let segments: Vec<&str> = path.trim_matches('/').split('/').filter(|s| !s.is_empty()).collect();
    match segments.as_slice() {
        ["view"] => submit_view(state, fields),
        ["add"] => submit_add(state, fields),
        ["print"] => submit_print(state, fields),
        ["measurement", id, "delete"] => submit_delete_measurement(state, id, fields),
        ["instruments", "add"] => submit_instrument_add(state, fields),
        ["instruments", id, "delete"] => submit_instrument_delete(state, id),
        ["instruments", id, "edit"] => submit_instrument_edit(state, id, fields),
        ["instruments", iid, "procedures", "add"] => submit_mode_add(state, iid, fields),
        ["instruments", iid, "procedures", mid, "edit"] => submit_mode_edit(state, iid, mid, fields),
        ["instruments", iid, "procedures", mid, "delete"] => submit_mode_delete(state, iid, mid),
        ["instruments", iid, "procedures", mid, "move"] => submit_mode_move(state, iid, mid, fields),
        ["batch", "measure"] => submit_batch_measure(state, fields),
        ["batch", "location"] => submit_batch_location(state, fields),
        ["match"] => submit_match(state, fields),
        ["match", id, "delete"] => submit_delete_match(state, id),
        ["match", id, "return"] => submit_return_match(state, id),
        ["history", id, "delete"] => submit_delete_location_entry(state, id),
        ["settings"] => submit_settings(state, fields),
        ["settings", "reset"] => submit_settings_reset(state),
        ["b", id, "location"] => submit_location(state, id, fields),
        ["b", id, "edit"] => submit_edit(state, id, fields),
        ["b", id, "delete"] => submit_delete(state, id, fields),
        ["b", id, "purge"] => submit_purge(state, id, fields),
        ["b", id, "restore"] => submit_restore(state, id, fields),
        ["b", id, "reset"] => submit_reset(state, id, fields),
        ["b", id, "measure"] => submit_measure(state, id, fields),
        ["b", id, "print"] => submit_print_one(state, id, fields),
        _ => redirect("/", &[("error", "Unknown action.")]),
    }
}

fn submit_view(state: &AppState, fields: &HashMap<String, String>) -> CommandResult {
    let db = state.db.lock().unwrap();
    let view_value = get_str(fields, "view");
    if view_value == "table" || view_value == "grid" {
        let _ = config::save(&db, "list_view", view_value);
    }
    let next = safe_path(get_str(fields, "next"), "/");
    redirect(next, &[])
}

fn submit_add(state: &AppState, fields: &HashMap<String, String>) -> CommandResult {
    let db = state.db.lock().unwrap();
    let count = parse_positive_int(get_str(fields, "count")).unwrap_or(1).clamp(1, 200);
    let type_name = get_str(fields, "type").trim();
    if type_name.is_empty() {
        drop(db);
        return page_add(state, "", "A type is required.", Some(json!({"add_values": fields_to_value(fields)})));
    }
    let start_id = match parse_positive_int(get_str(fields, "battery_id")) {
        Some(id) => id,
        None => db.next_id().unwrap_or(1),
    };
    let ids: Vec<i64> = (start_id..start_id + count).collect();
    let in_use = db.ids_in_use(&ids).unwrap_or_default();
    if !in_use.is_empty() {
        let list = in_use.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(", ");
        drop(db);
        return page_add(
            state,
            "",
            &format!("Id(s) already in use: {list}."),
            Some(json!({"add_values": fields_to_value(fields)})),
        );
    }

    let nominal_mah = whole_number(get_str(fields, "nominal_mah"));
    let brand = get_str(fields, "brand");
    let location = get_str(fields, "location");
    let mut created = Vec::new();
    for id in &ids {
        match db.create(*id, type_name, nominal_mah, brand, location, "") {
            Ok(b) => created.push(b),
            Err(e) => {
                drop(db);
                return page_add(state, "", &e.to_string(), Some(json!({"add_values": fields_to_value(fields)})));
            }
        }
    }

    let want_print = fields.contains_key("print_label");
    let ids_i64: Vec<i64> = created.iter().map(|b| b.id).collect();
    // In PDF mode there's no in-app printer to send to — the redirect below
    // carries the created ids instead, and app.js triggers the save dialog
    // once it lands, the same as every other print action in PDF mode.
    let save_pdf_ids = if want_print && label_output_is_pdf(&db) {
        ids_i64.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",")
    } else {
        String::new()
    };
    if want_print && save_pdf_ids.is_empty() && labels::printer_status(&db).available {
        let tape_mm_x10 = tape_mm_x10(&db);
        let _ = labels::print_labels(&db, &ids_i64, tape_mm_x10);
    }

    if created.len() == 1 {
        redirect(format!("/b/{}", created[0].id), &[("msg", "Battery added."), ("save_pdf_ids", &save_pdf_ids)])
    } else {
        let next = safe_path(get_str(fields, "next"), "/");
        redirect(next, &[("msg", &format!("{} batteries added.", created.len())), ("save_pdf_ids", &save_pdf_ids)])
    }
}

/// Re-rendering a form with an error carries the submitted values back into
/// the template as `Value`s so `{% if i.id == some_field %}`-style selected/
/// checked comparisons keep working: `i.id` is a real JSON number, so a
/// submitted id field has to become one too, not stay the string every HTML
/// form field arrives as — otherwise `5 == "5"` is false in Tera and the
/// previous selection silently reverts to nothing on every validation error.
fn fields_to_value(fields: &HashMap<String, String>) -> Value {
    let map: serde_json::Map<String, Value> = fields
        .iter()
        .map(|(k, v)| {
            let value = match v.parse::<i64>() {
                Ok(n) => json!(n),
                Err(_) => json!(v),
            };
            (k.clone(), value)
        })
        .collect();
    Value::Object(map)
}

/// Rebuilds `batch_measure.html`'s per-row table values from the submitted
/// form fields, for re-rendering the page with an error without losing
/// everything the user typed into all 16 (or 10, for "bought") rows.
fn batch_rows_from_fields(fields: &HashMap<String, String>, max_row: i64, bought: bool) -> Vec<Value> {
    (1..=max_row)
        .map(|i| {
            if bought {
                json!({"i": i, "battery_id": get_str(fields, &format!("battery_id_{i}"))})
            } else {
                json!({
                    "i": i,
                    "battery_id": get_str(fields, &format!("battery_id_{i}")),
                    "capacity_mah": get_str(fields, &format!("capacity_mah_{i}")),
                    "ir_mohm": get_str(fields, &format!("ir_mohm_{i}")),
                    "discharge_ma": get_str(fields, &format!("discharge_ma_{i}")),
                    "charge_ma": get_str(fields, &format!("charge_ma_{i}")),
                    "notes": get_str(fields, &format!("notes_{i}")),
                })
            }
        })
        .collect()
}

fn submit_print(state: &AppState, fields: &HashMap<String, String>) -> CommandResult {
    let db = state.db.lock().unwrap();
    let spec = get_str(fields, "spec").to_string();
    let confirm = get_str(fields, "confirm") == "yes";

    let ids = match parse_id_spec(&spec) {
        Ok(ids) => ids,
        Err(e) => {
            drop(db);
            return page_print(state, "", &e, Some(json!({"print_spec": spec})));
        }
    };

    let mut resolved = Vec::new();
    let mut missing = Vec::new();
    let mut deleted = Vec::new();
    for id in ids {
        match db.get(id).unwrap_or(None) {
            None => missing.push(id.to_string()),
            Some(b) if b.deleted_at.is_some() => deleted.push(id.to_string()),
            Some(b) => resolved.push(b),
        }
    }

    if confirm && !resolved.is_empty() {
        let ids_i64: Vec<i64> = resolved.iter().map(|b| b.id).collect();
        if label_output_is_pdf(&db) {
            drop(db);
            let ids_csv: String = ids_i64.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",");
            return redirect("/print", &[("save_pdf_ids", &ids_csv)]);
        }
        let tape_mm_x10 = tape_mm_x10(&db);
        if let Err(e) = labels::print_labels(&db, &ids_i64, tape_mm_x10) {
            drop(db);
            return page_print(state, "", &e, Some(json!({"print_spec": spec})));
        }
        drop(db);
        return redirect("/print", &[("msg", &format!("{} label(s) printed.", resolved.len()))]);
    }

    drop(db);
    page_print(
        state,
        "",
        "",
        Some(json!({
            "print_spec": spec,
            "print_resolved": resolved,
            "print_missing": missing,
            "print_deleted": deleted,
        })),
    )
}

fn submit_location(state: &AppState, id: &str, fields: &HashMap<String, String>) -> CommandResult {
    let Ok(id) = id.parse::<i64>() else { return redirect("/", &[]) };
    let db = state.db.lock().unwrap();
    let location = get_str(fields, "location");
    let pool_location = config::get(&db, "pool_location").as_text();
    match db.set_location_checked(id, location, &pool_location) {
        Ok((Some(_), also_returned)) => {
            drop(db);
            let mut msg = "Location saved.".to_string();
            if !also_returned.is_empty() {
                msg.push_str(&format!(" Also returned from the same match: {}.", id_list(&also_returned)));
            }
            redirect(format!("/b/{id}"), &[("msg", &msg)])
        }
        _ => redirect(format!("/b/{id}"), &[("error", "Could not save the location.")]),
    }
}

fn submit_edit(state: &AppState, id: &str, fields: &HashMap<String, String>) -> CommandResult {
    let Ok(id) = id.parse::<i64>() else { return redirect("/", &[]) };
    let db = state.db.lock().unwrap();
    let type_name = get_str(fields, "type");
    let nominal_mah = Some(whole_number(get_str(fields, "nominal_mah")));
    let brand = get_str(fields, "brand");
    let notes = get_str(fields, "notes");
    let _ = db.update(id, Some(type_name), nominal_mah, Some(brand), Some(notes));
    drop(db);
    redirect(format!("/b/{id}"), &[("tab", "maintenance"), ("msg", "Details saved.")])
}

fn submit_delete(state: &AppState, id: &str, fields: &HashMap<String, String>) -> CommandResult {
    let Ok(id) = id.parse::<i64>() else { return redirect("/", &[]) };
    let db = state.db.lock().unwrap();
    let next = safe_path(get_str(fields, "next"), "/");
    let _ = db.soft_delete(id);
    drop(db);
    redirect(next, &[("msg", &format!("Battery {id:03} deleted."))])
}

fn submit_purge(state: &AppState, id: &str, fields: &HashMap<String, String>) -> CommandResult {
    let Ok(id) = id.parse::<i64>() else { return redirect("/", &[]) };
    let db = state.db.lock().unwrap();
    let next = safe_path(get_str(fields, "next"), "/");
    match db.purge(id) {
        Ok(()) => {
            drop(db);
            redirect(next, &[("msg", &format!("Battery {id:03} permanently deleted."))])
        }
        Err(e) => {
            drop(db);
            redirect(format!("/b/{id}"), &[("tab", "maintenance"), ("error", &e)])
        }
    }
}

fn submit_restore(state: &AppState, id: &str, fields: &HashMap<String, String>) -> CommandResult {
    let Ok(id) = id.parse::<i64>() else { return redirect("/", &[]) };
    let db = state.db.lock().unwrap();
    let next = safe_path(get_str(fields, "next"), "/");
    let _ = db.restore(id);
    drop(db);
    redirect(next, &[("msg", &format!("Battery {id:03} restored."))])
}

fn submit_reset(state: &AppState, id: &str, fields: &HashMap<String, String>) -> CommandResult {
    let Ok(id) = id.parse::<i64>() else { return redirect("/", &[]) };
    let db = state.db.lock().unwrap();
    let next = safe_path(get_str(fields, "next"), "/");
    let _ = db.reset(id);
    drop(db);
    redirect(next, &[("msg", &format!("Battery {id:03} cleared."))])
}

fn submit_measure(state: &AppState, id: &str, fields: &HashMap<String, String>) -> CommandResult {
    let Ok(id) = id.parse::<i64>() else { return redirect("/", &[]) };
    let db = state.db.lock().unwrap();
    let kind = get_str(fields, "kind");
    let measured_at = get_str(fields, "measured_at");
    let instrument_id = whole_number(get_str(fields, "instrument_id"));
    let mode_id = whole_number(get_str(fields, "mode_id"));
    let notes = get_str(fields, "notes");

    let parsed = (|| -> Result<_, String> {
        Ok((
            unsigned_number(get_str(fields, "capacity_mah"), "Capacity")?,
            unsigned_number(get_str(fields, "ir_mohm"), "Resistance")?,
            unsigned_number(get_str(fields, "discharge_ma"), "Discharge current")?,
            unsigned_number(get_str(fields, "charge_ma"), "Charge current")?,
        ))
    })();
    let (capacity_mah, ir_mohm, discharge_ma, charge_ma) = match parsed {
        Ok(v) => v,
        Err(e) => {
            drop(db);
            return page_detail_with_measure_error(state, id, &e, fields);
        }
    };

    if let Err(e) = clean_reading(&db, kind, capacity_mah, ir_mohm, instrument_id, mode_id, discharge_ma, charge_ma) {
        drop(db);
        return page_detail_with_measure_error(state, id, &e, fields);
    }

    if kind == "bought" {
        // A fresh "bought" entry replaces any existing one: a purchase is a
        // single fact about a battery, not a log of repeated events.
        for m in db.measurements(id).unwrap_or_default() {
            if m.kind == "bought" {
                let _ = db.delete_measurement(m.id);
            }
        }
    }

    match db.add_measurement(id, kind, measured_at, capacity_mah, ir_mohm, instrument_id, mode_id, discharge_ma, charge_ma, notes) {
        Ok(Some(_)) => {
            drop(db);
            redirect(format!("/b/{id}"), &[("msg", "Reading saved.")])
        }
        _ => {
            drop(db);
            page_detail_with_measure_error(state, id, "That battery no longer exists.", fields)
        }
    }
}

fn page_detail_with_measure_error(state: &AppState, id: i64, error: &str, fields: &HashMap<String, String>) -> CommandResult {
    let db = state.db.lock().unwrap();
    let battery = match db.get(id).unwrap_or(None) {
        Some(b) => b,
        None => return page(state, "missing.html", json!({"battery_id": id}), &format!("Battery {id}"), "", "", ""),
    };
    let (prev, next) = db.adjacent_ids(id).unwrap_or((None, None));
    let measurements = db.measurements(id).unwrap_or_default();
    let history = db.history(id).unwrap_or_default();
    let limits = view::ir_limits(&db);
    let health_limit = Some(config::get(&db, "health_limit_pct").as_i64());
    let names = view::instrument_names(&db);
    let modes = view::mode_names(&db);
    let condition = view::condition_view(&battery, &measurements, &limits, health_limit, &names);
    let readings: Vec<Value> = measurements.iter().map(|m| view::reading_row(m, &names, &modes)).collect();
    let instruments: Vec<Value> = db.list_instruments().unwrap_or_default().iter().filter(|i| !i.modes.is_empty()).map(view::instrument_with_modes).collect();
    let types = db.distinct("type").unwrap_or_default();
    let brands = db.distinct("brand").unwrap_or_default();
    let locations = db.distinct("location").unwrap_or_default();
    let top_locations = db.popular("location", 8).unwrap_or_default();

    let ctx = json!({
        "battery": battery, "adjacent": {"prev": prev, "next": next}, "tab": "",
        "condition": condition, "history": history, "readings": readings,
        "instruments": instruments, "kinds": batteries_core::db::KINDS, "today": today(),
        "types": types, "brands": brands, "locations": locations, "top_locations": top_locations,
        "measure_error": error,
        "measure": fields_to_value(fields),
    });
    let title = format!("Battery {id:03}");
    page(state, "detail.html", ctx, &title, "", "", "measure")
}

fn submit_print_one(state: &AppState, id: &str, _fields: &HashMap<String, String>) -> CommandResult {
    let Ok(id) = id.parse::<i64>() else { return redirect("/", &[]) };
    let db = state.db.lock().unwrap();
    if label_output_is_pdf(&db) {
        drop(db);
        return redirect(format!("/b/{id}"), &[("tab", "maintenance"), ("save_pdf_ids", &id.to_string())]);
    }
    let tape_mm_x10 = tape_mm_x10(&db);
    match labels::print_labels(&db, &[id], tape_mm_x10) {
        Ok(()) => {
            drop(db);
            redirect(format!("/b/{id}"), &[("tab", "maintenance"), ("msg", "Label sent to the printer.")])
        }
        Err(e) => {
            drop(db);
            redirect(format!("/b/{id}"), &[("tab", "maintenance"), ("error", &e)])
        }
    }
}

fn submit_delete_measurement(state: &AppState, id: &str, _fields: &HashMap<String, String>) -> CommandResult {
    let Ok(id) = id.parse::<i64>() else { return redirect("/", &[]) };
    let db = state.db.lock().unwrap();
    match db.delete_measurement(id) {
        Ok(Some(battery_id)) => {
            drop(db);
            redirect(format!("/b/{battery_id}"), &[("msg", "Reading deleted.")])
        }
        _ => redirect("/", &[]),
    }
}

fn submit_delete_location_entry(state: &AppState, id: &str) -> CommandResult {
    let Ok(id) = id.parse::<i64>() else { return redirect("/", &[]) };
    let db = state.db.lock().unwrap();
    match db.delete_location_entry(id) {
        Ok(Some(battery_id)) => {
            drop(db);
            redirect(format!("/b/{battery_id}"), &[("msg", "Location entry deleted.")])
        }
        _ => redirect("/", &[]),
    }
}

fn submit_delete_match(state: &AppState, id: &str) -> CommandResult {
    let Ok(id) = id.parse::<i64>() else { return redirect("/match/log", &[]) };
    let db = state.db.lock().unwrap();
    let deleted = db.delete_match(id).unwrap_or(false);
    drop(db);
    if deleted {
        redirect("/match/log", &[("msg", "Match deleted.")])
    } else {
        redirect("/match/log", &[("error", "That match no longer exists.")])
    }
}

fn submit_return_match(state: &AppState, id: &str) -> CommandResult {
    let Ok(id) = id.parse::<i64>() else { return redirect("/match/log", &[]) };
    let db = state.db.lock().unwrap();
    let pool_location = config::get(&db, "pool_location").as_text();
    let result = db.return_match(id, &pool_location);
    drop(db);
    match result {
        Ok(moved) if moved.is_empty() => redirect("/match/log", &[("msg", "Match closed.")]),
        Ok(moved) => redirect(
            "/match/log",
            &[("msg", &format!("Returned {} to {pool_location}.", id_list(&moved)))],
        ),
        Err(_) => redirect("/match/log", &[("error", "Could not return that match.")]),
    }
}

fn submit_instrument_add(state: &AppState, fields: &HashMap<String, String>) -> CommandResult {
    let db = state.db.lock().unwrap();
    let name = get_str(fields, "name");
    let slots = parse_positive_int(get_str(fields, "slots")).unwrap_or(1);
    match db.add_instrument(name, slots) {
        Ok(_) => {
            drop(db);
            redirect("/instruments", &[("msg", "Instrument added.")])
        }
        Err(e) => {
            drop(db);
            page_instruments(
                state, &HashMap::new(), "", "",
                Some(json!({"add_instrument_open": true, "add_instrument_error": e.to_string(), "add_instrument_values": fields_to_value(fields)})),
            )
        }
    }
}

fn submit_instrument_edit(state: &AppState, id: &str, fields: &HashMap<String, String>) -> CommandResult {
    let Ok(id) = id.parse::<i64>() else { return redirect("/instruments", &[]) };
    let db = state.db.lock().unwrap();
    let name = get_str(fields, "name");
    let slots = parse_positive_int(get_str(fields, "slots")).unwrap_or(1);
    match db.update_instrument(id, name, slots) {
        Ok(Some(_)) => {
            drop(db);
            redirect("/instruments", &[("msg", "Instrument saved.")])
        }
        Ok(None) => redirect("/instruments", &[]),
        Err(e) => {
            drop(db);
            page_instruments(
                state, &HashMap::new(), "", "",
                Some(json!({"edit_instrument_for": id, "edit_instrument_error": e.to_string(), "edit_instrument_values": fields_to_value(fields)})),
            )
        }
    }
}

fn submit_instrument_delete(state: &AppState, id: &str) -> CommandResult {
    let Ok(id) = id.parse::<i64>() else { return redirect("/instruments", &[]) };
    let db = state.db.lock().unwrap();
    let _ = db.delete_instrument(id);
    drop(db);
    redirect("/instruments", &[("msg", "Instrument deleted.")])
}

fn field_pair(fields: &HashMap<String, String>, name: &str) -> Value {
    json!({
        "visible": fields.contains_key(&format!("field_{name}_visible")),
        "required": fields.contains_key(&format!("field_{name}_required")),
    })
}

fn fields_json_from_form(fields: &HashMap<String, String>) -> Value {
    json!({
        "capacity_mah": field_pair(fields, "capacity_mah"),
        "ir_mohm": field_pair(fields, "ir_mohm"),
        "discharge_ma": field_pair(fields, "discharge_ma"),
        "charge_ma": field_pair(fields, "charge_ma"),
        "notes": field_pair(fields, "notes"),
    })
}

fn submit_mode_add(state: &AppState, iid: &str, fields: &HashMap<String, String>) -> CommandResult {
    let Ok(iid) = iid.parse::<i64>() else { return redirect("/instruments", &[]) };
    let db = state.db.lock().unwrap();
    let name = get_str(fields, "name");
    let kind = get_str(fields, "kind");
    let details = get_str(fields, "details");
    let mode_fields = fields_json_from_form(fields);
    match db.add_mode(iid, name, kind, details, Some(&mode_fields)) {
        Ok(_) => {
            drop(db);
            redirect("/instruments", &[("msg", "Procedure added.")])
        }
        Err(e) => {
            drop(db);
            let mut values = fields_to_value(fields);
            if let Value::Object(m) = &mut values {
                m.insert("fields_dict".into(), mode_fields);
            }
            page_instruments(
                state, &HashMap::new(), "", "",
                Some(json!({"add_mode_for": iid, "add_mode_error": e.to_string(), "add_mode_values": values})),
            )
        }
    }
}

fn submit_mode_edit(state: &AppState, iid: &str, mid: &str, fields: &HashMap<String, String>) -> CommandResult {
    let Ok(mid) = mid.parse::<i64>() else { return redirect("/instruments", &[]) };
    let iid = iid.parse::<i64>().unwrap_or_default();
    let db = state.db.lock().unwrap();
    let name = get_str(fields, "name");
    let kind = get_str(fields, "kind");
    let details = get_str(fields, "details");
    let mode_fields = fields_json_from_form(fields);
    match db.update_mode(mid, name, kind, details, Some(&mode_fields)) {
        Ok(Some(_)) => {
            drop(db);
            // Deliberately no `open` param here: saving closes the card back
            // to view mode (unlike moving, which keeps it open — see
            // submit_mode_move below), matching the original app's behavior.
            redirect("/instruments", &[("msg", "Procedure saved.")])
        }
        Ok(None) => redirect("/instruments", &[]),
        Err(e) => {
            drop(db);
            page_instruments(
                state, &HashMap::new(), "", "",
                Some(json!({"edit_mode_for": mid, "edit_mode_error": e.to_string(), "edit_mode_values": fields_to_value(fields), "_iid": iid})),
            )
        }
    }
}

fn submit_mode_delete(state: &AppState, _iid: &str, mid: &str) -> CommandResult {
    let Ok(mid) = mid.parse::<i64>() else { return redirect("/instruments", &[]) };
    let db = state.db.lock().unwrap();
    let _ = db.delete_mode(mid);
    drop(db);
    redirect("/instruments", &[("msg", "Procedure deleted.")])
}

fn submit_mode_move(state: &AppState, _iid: &str, mid: &str, fields: &HashMap<String, String>) -> CommandResult {
    let Ok(mid) = mid.parse::<i64>() else { return redirect("/instruments", &[]) };
    let db = state.db.lock().unwrap();
    let direction = get_str(fields, "direction");
    let _ = db.move_mode(mid, direction);
    drop(db);
    // The reorder buttons sit in the header row now, not behind the edit
    // disclosure, so there is no form left open to preserve here — just
    // scroll back to the card that moved instead of resetting to the top.
    redirect(format!("/instruments#procedure-{mid}"), &[])
}

fn submit_batch_measure(state: &AppState, fields: &HashMap<String, String>) -> CommandResult {
    let db = state.db.lock().unwrap();
    let kind = get_str(fields, "kind");
    let measured_at = get_str(fields, "measured_at");
    let instrument_id = whole_number(get_str(fields, "instrument_id"));
    let mode_id = whole_number(get_str(fields, "mode_id"));

    let mut saved = 0;
    let mut missing: Vec<String> = Vec::new();
    let max_row = if kind == "bought" { 10 } else { 16 };
    for i in 1..=max_row {
        let battery_id = match parse_positive_int(get_str(fields, &format!("battery_id_{i}"))) {
            Some(id) => id,
            None => continue,
        };
        if db.get(battery_id).unwrap_or(None).is_none() {
            missing.push(battery_id.to_string());
            continue;
        }
        let notes = get_str(fields, &format!("notes_{i}"));
        let parsed = (|| -> Result<_, String> {
            Ok((
                unsigned_number(get_str(fields, &format!("capacity_mah_{i}")), "Capacity")?,
                unsigned_number(get_str(fields, &format!("ir_mohm_{i}")), "Resistance")?,
                unsigned_number(get_str(fields, &format!("discharge_ma_{i}")), "Discharge current")?,
                unsigned_number(get_str(fields, &format!("charge_ma_{i}")), "Charge current")?,
            ))
        })();
        let (capacity_mah, ir_mohm, discharge_ma, charge_ma) = match parsed {
            Ok(v) => v,
            Err(e) => {
                drop(db);
                return page_batch_measure(
                    state, "", &format!("Battery {battery_id}: {e}"),
                    Some(json!({
                        "common": fields_to_value(fields),
                        "slot_rows": batch_rows_from_fields(fields, 16, false),
                        "bought_rows": batch_rows_from_fields(fields, 10, true),
                    })),
                );
            }
        };

        if let Err(e) = clean_reading(&db, kind, capacity_mah, ir_mohm, instrument_id, mode_id, discharge_ma, charge_ma) {
            drop(db);
            return page_batch_measure(
                state, "", &format!("Battery {battery_id}: {e}"),
                Some(json!({
                    "common": fields_to_value(fields),
                    "slot_rows": batch_rows_from_fields(fields, 16, false),
                    "bought_rows": batch_rows_from_fields(fields, 10, true),
                })),
            );
        }
        let _ = db.add_measurement(battery_id, kind, measured_at, capacity_mah, ir_mohm, instrument_id, mode_id, discharge_ma, charge_ma, notes);
        saved += 1;
    }

    drop(db);
    if saved == 0 && missing.is_empty() {
        page_batch_measure(state, "", "No battery ids were given.", None)
    } else if saved == 0 {
        page_batch_measure(
            state, "",
            &format!("No battery has ever had id(s) {}.", missing.join(", ")),
            Some(json!({
                "common": fields_to_value(fields),
                "slot_rows": batch_rows_from_fields(fields, 16, false),
                "bought_rows": batch_rows_from_fields(fields, 10, true),
            })),
        )
    } else {
        let mut msg = format!("{saved} reading(s) recorded.");
        if !missing.is_empty() {
            msg.push_str(&format!(" Skipped (no such battery): {}.", missing.join(", ")));
        }
        redirect("/batch/measure", &[("msg", &msg)])
    }
}

fn submit_batch_location(state: &AppState, fields: &HashMap<String, String>) -> CommandResult {
    let db = state.db.lock().unwrap();
    let spec = get_str(fields, "spec").to_string();
    let location = get_str(fields, "location").to_string();
    let ids = match parse_id_spec(&spec) {
        Ok(ids) => ids,
        Err(e) => {
            drop(db);
            return page_batch_location(state, &HashMap::new(), "", &e, Some(json!({"spec": spec, "location": location})));
        }
    };

    let pool_location = config::get(&db, "pool_location").as_text();
    let mut moved = 0;
    let mut skipped = Vec::new();
    let mut also_returned: Vec<i64> = Vec::new();
    for &id in &ids {
        match db.get(id).unwrap_or(None) {
            Some(b) if b.deleted_at.is_none() => {
                if let Ok((_, others)) = db.set_location_checked(id, &location, &pool_location) {
                    also_returned.extend(others);
                }
                moved += 1;
            }
            _ => skipped.push(id.to_string()),
        }
    }
    also_returned.retain(|other| !ids.contains(other));
    also_returned.sort_unstable();
    also_returned.dedup();

    drop(db);
    let mut msg = format!("{moved} batter{} moved.", if moved == 1 { "y" } else { "ies" });
    if !also_returned.is_empty() {
        msg.push_str(&format!(" Also returned from the same match: {}.", id_list(&also_returned)));
    }
    if !skipped.is_empty() {
        msg.push_str(&format!(" Skipped: {}.", skipped.join(", ")));
    }
    redirect("/batch/location", &[("msg", &msg)])
}

fn submit_match(state: &AppState, fields: &HashMap<String, String>) -> CommandResult {
    let db = state.db.lock().unwrap();
    let requested_type = get_str(fields, "type").trim().to_uppercase();
    let count_text = get_str(fields, "count").to_string();
    let draw = matching::Draw::parse(get_str(fields, "draw")).unwrap_or(matching::Draw::Low);
    let location = get_str(fields, "location").trim().to_string();
    let confirm = get_str(fields, "confirm") == "yes";

    let echo = json!({
        "match_type": requested_type, "match_count": count_text,
        "match_draw": draw.as_str(), "match_location": location,
    });

    if requested_type.is_empty() {
        drop(db);
        return page_match(state, "", "Enter a battery type.", Some(echo));
    }
    if location.is_empty() {
        drop(db);
        return page_match(state, "", "Enter a destination location.", Some(echo));
    }
    let count = match parse_positive_int(&count_text) {
        Some(n) => n as usize,
        None => {
            drop(db);
            return page_match(state, "", "Enter how many batteries you need, as a whole number.", Some(echo));
        }
    };

    let pool_location = config::get(&db, "pool_location").as_text();
    let default_ir = config::get(&db, "default_ir_mohm").as_i64();
    let all = db.list_batteries("", "", "", "active", "id", None, 0).unwrap_or_default();
    let mut measurements_by_battery = HashMap::new();
    for b in all.iter().filter(|b| b.location.eq_ignore_ascii_case(&pool_location)) {
        measurements_by_battery.insert(b.id, db.measurements(b.id).unwrap_or_default());
    }
    let candidates = matching::eligible_pool(&all, &measurements_by_battery, &pool_location, default_ir);

    let suggestion = match matching::suggest(&candidates, &requested_type, count, draw) {
        Ok(s) => s,
        Err(e) => {
            drop(db);
            return page_match(state, "", &e, Some(echo));
        }
    };

    if confirm {
        let _ = db.create_match(&location, &requested_type, draw.as_str(), &suggestion.battery_ids);
        drop(db);
        let n = suggestion.battery_ids.len();
        return redirect(
            "/match",
            &[(
                "msg",
                &format!("{n} \"{requested_type}\" batter{} moved to {location}.", if n == 1 { "y" } else { "ies" }),
            )],
        );
    }

    let by_id: HashMap<i64, &matching::Candidate> = candidates.iter().map(|c| (c.battery.id, c)).collect();
    let rows: Vec<Value> = suggestion
        .battery_ids
        .iter()
        .filter_map(|id| {
            let c = by_id.get(id)?;
            Some(json!({
                "id": c.battery.id, "type": c.battery.r#type, "brand": c.battery.brand,
                "nominal_mah": c.battery.nominal_mah, "capacity_mah": c.capacity_mah,
                "ir_mohm": c.ir_mohm, "ir_assumed": c.ir_assumed,
            }))
        })
        .collect();

    drop(db);
    let mut ctx = echo;
    if let Value::Object(map) = &mut ctx {
        map.insert(
            "suggestion".into(),
            json!({ "rows": rows, "mixed": suggestion.mixed, "count": suggestion.battery_ids.len() }),
        );
    }
    page_match(state, "", "", Some(ctx))
}

fn submit_settings(state: &AppState, fields: &HashMap<String, String>) -> CommandResult {
    let db = state.db.lock().unwrap();
    for spec in config::SETTINGS {
        let raw = if spec.kind == config::Kind::Bool {
            if fields.contains_key(spec.key) { "1" } else { "0" }
        } else {
            get_str(fields, spec.key)
        };
        if let Err(e) = config::save(&db, spec.key, raw) {
            drop(db);
            return page_settings(state, "", &e, None);
        }
    }
    drop(db);
    redirect("/settings", &[("msg", "Settings saved.")])
}

fn submit_settings_reset(state: &AppState) -> CommandResult {
    let db = state.db.lock().unwrap();
    for spec in config::SETTINGS {
        let _ = config::clear(&db, spec.key);
    }
    drop(db);
    redirect("/settings", &[("msg", "All overrides cleared.")])
}
