//! Router-level integration tests: call `render_page`/`submit_form` exactly
//! as `dist/app.js` does over IPC, against a throwaway per-test database, and
//! assert on both the database state and the actual HTML the webview would
//! receive — no browser, no WebView2, no UI Automation involved. Each test
//! opens its own `tempfile::tempdir()` database, the same isolation pattern
//! `core`'s own tests use, so nothing here ever touches the real
//! `%APPDATA%\com.batteries.app\batteries.db`.
//!
//! This does not exercise client-side JS (form interception, confirm
//! dialogs, ui.js's dynamic show/hide) — only what the Rust side renders and
//! does in response to a path/query or a path/fields submission.
//!
//! Each test below is a regression test for a specific bug fixed this
//! project: see the comment on each for which one.

use std::collections::HashMap;
use std::sync::Mutex;

use batteries_core::db::Db;

use battery_log_lib::router::{self, CommandResult};
use battery_log_lib::state::AppState;
use battery_log_lib::templates;

fn fresh_state() -> (tempfile::TempDir, AppState) {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = Db::open(&dir.path().join("test.db")).expect("open test db");
    let tera = templates::build();
    (dir, AppState { db: Mutex::new(db), tera })
}

fn fields(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

fn expect_page(result: CommandResult) -> String {
    match result {
        CommandResult::Page { html, .. } => html,
        CommandResult::Redirect { path, query } => {
            panic!("expected a page, got a redirect to {path:?}?{query:?}")
        }
    }
}

fn expect_redirect(result: CommandResult) -> (String, Option<String>) {
    match result {
        CommandResult::Redirect { path, query } => (path, query),
        CommandResult::Page { .. } => panic!("expected a redirect, got a page"),
    }
}

// ── Scope pills only appear once there's something for them to distinguish ──

#[test]
fn scope_pills_hidden_with_nothing_deleted_or_flagged() {
    let (_dir, state) = fresh_state();
    state.db.lock().unwrap().create(1, "AA", Some(2000), "", "", "").unwrap();

    let html = expect_page(router::render_page(&state, "/", ""));

    assert!(!html.contains(">Deleted<"), "nothing deleted yet, the pill shouldn't render");
    assert!(!html.contains(">Active<"), "nothing to distinguish 'active' from 'all' yet");
}

#[test]
fn deleted_pill_appears_once_something_is_deleted() {
    let (_dir, state) = fresh_state();
    {
        let db = state.db.lock().unwrap();
        db.create(1, "AA", Some(2000), "", "", "").unwrap();
        db.create(2, "AA", Some(2000), "", "", "").unwrap();
        db.soft_delete(1).unwrap();
    }

    let html = expect_page(router::render_page(&state, "/", ""));

    assert!(html.contains(">Deleted<"), "the Deleted pill should show once something is deleted");
    assert!(html.contains(">Active<"), "and the Active pill alongside it");
}

// ── The list's heading tallies batteries instead of counting them ───────

/// 3 active AA, 2 active AAA, and a deleted AAA that mustn't be counted.
fn seed_types(state: &AppState) {
    let db = state.db.lock().unwrap();
    for id in 1..=3 {
        db.create(id, "AAA", Some(800), "", "", "").unwrap();
    }
    for id in 4..=6 {
        db.create(id, "AA", Some(2000), "", "", "").unwrap();
    }
    db.soft_delete(3).unwrap();
}

#[test]
fn heading_lists_active_batteries_by_type() {
    let (_dir, state) = fresh_state();
    seed_types(&state);

    let html = expect_page(router::render_page(&state, "/", ""));

    assert!(html.contains(r#"<p class="sub">3x AA, 2x AAA</p>"#), "heading should tally active types: {html}");
}

#[test]
fn heading_for_a_type_filter_counts_active_flagged_and_deleted() {
    let (_dir, state) = fresh_state();
    seed_types(&state);
    {
        // 50% of nominal is well under the default 80% health limit.
        let db = state.db.lock().unwrap();
        db.add_measurement(4, "analysed", "2026-01-01", Some(1000), None, None, None, Some(500), None, "")
            .unwrap();
        db.create(7, "AA", Some(2000), "", "", "").unwrap();
        db.soft_delete(5).unwrap();
    }

    let html = expect_page(router::render_page(&state, "/", "type=AA"));

    assert!(
        html.contains(r#"<p class="sub">3x active, 1x flagged, 1x deleted</p>"#),
        "heading should summarise the type across every status: {html}"
    );
}

#[test]
fn heading_for_a_type_filter_omits_statuses_with_nothing_in_them() {
    let (_dir, state) = fresh_state();
    seed_types(&state);

    let html = expect_page(router::render_page(&state, "/", "type=AAA"));

    assert!(
        html.contains(r#"<p class="sub">2x active, 1x deleted</p>"#),
        "no flagged AAA, so no flagged part: {html}"
    );
}

// ── Regression: deleting from a filtered list used to panic ─────────────
// `redirect()` had a `debug_assert!` rejecting any `path` that carried its
// own `?query` — but `next`/`list_url` legitimately do, by design, so a
// delete/restore/reset submitted with a `next` field like this one crashed
// the whole app. The fix made `redirect()` split-and-merge instead.

#[test]
fn deleting_from_a_filtered_list_does_not_panic_and_preserves_the_filter() {
    let (_dir, state) = fresh_state();
    {
        let db = state.db.lock().unwrap();
        db.create(1, "AA", Some(2000), "", "", "").unwrap();
        db.create(2, "AA", Some(2000), "", "", "").unwrap();
    }

    let result = router::submit_form(
        &state,
        "/b/1/delete",
        &fields(&[("next", "/?show=active&type=AA")]),
    );

    let (path, query) = expect_redirect(result);
    assert_eq!(path, "/");
    let query = query.unwrap_or_default();
    assert!(query.contains("type=AA"), "filter should survive the redirect: {query:?}");
    assert!(query.contains("show=active"), "scope should survive the redirect: {query:?}");
}

// ── Purge: only ever for a deleted battery with no readings ─────────────

#[test]
fn purge_removes_a_deleted_battery_with_no_readings() {
    let (_dir, state) = fresh_state();
    {
        let db = state.db.lock().unwrap();
        db.create(1, "AA", None, "", "", "").unwrap();
        db.create(2, "AA", None, "", "", "").unwrap(); // keeps 1 from auto-hard-deleting as "highest id"
        db.soft_delete(1).unwrap();
    }

    let result = router::submit_form(&state, "/b/1/purge", &fields(&[]));

    assert!(matches!(result, CommandResult::Redirect { .. }));
    assert!(state.db.lock().unwrap().get(1).unwrap().is_none(), "battery 1 should be gone entirely");
}

#[test]
fn purge_refuses_a_battery_that_has_readings() {
    let (_dir, state) = fresh_state();
    {
        let db = state.db.lock().unwrap();
        db.create(1, "AA", Some(2000), "", "", "").unwrap();
        db.create(2, "AA", Some(2000), "", "", "").unwrap();
        db.add_measurement(1, "analysed", "2026-01-01", Some(1900), None, None, None, Some(500), None, "")
            .unwrap();
        db.soft_delete(1).unwrap();
    }

    let err = state.db.lock().unwrap().purge(1);

    assert!(err.is_err(), "a battery with readings must not be purgeable");
    assert!(state.db.lock().unwrap().get(1).unwrap().is_some(), "battery 1 should still exist");
}

#[test]
fn purge_refuses_a_battery_that_is_not_deleted() {
    let (_dir, state) = fresh_state();
    state.db.lock().unwrap().create(1, "AA", None, "", "", "").unwrap();

    let err = state.db.lock().unwrap().purge(1);

    assert!(err.is_err(), "an active (non-deleted) battery must not be purgeable");
}

// ── Unsigned validation: negative readings are refused, value preserved ──

#[test]
fn negative_resistance_is_refused_with_the_typed_value_preserved() {
    let (_dir, state) = fresh_state();
    let instrument_id;
    {
        let db = state.db.lock().unwrap();
        db.create(1, "AA", Some(2000), "", "", "").unwrap();
        instrument_id = db.add_instrument("Tester", 1).unwrap().id;
        db.add_mode(instrument_id, "IR check", "analysed", "", None).unwrap();
    }

    let html = expect_page(router::submit_form(
        &state,
        "/b/1/measure",
        &fields(&[
            ("kind", "analysed"),
            ("measured_at", "2026-01-01"),
            ("instrument_id", &instrument_id.to_string()),
            ("ir_mohm", "-5"),
        ]),
    ));

    assert!(html.contains("cannot be negative"), "should refuse a negative reading");
    assert!(html.contains("-5"), "the typed value should be preserved, not reset");
}

#[test]
fn positive_readings_are_accepted() {
    let (_dir, state) = fresh_state();
    let instrument_id;
    {
        let db = state.db.lock().unwrap();
        db.create(1, "AA", Some(2000), "", "", "").unwrap();
        instrument_id = db.add_instrument("Tester", 1).unwrap().id;
        db.add_mode(instrument_id, "Full analysis", "analysed", "", None).unwrap();
    }

    let result = router::submit_form(
        &state,
        "/b/1/measure",
        &fields(&[
            ("kind", "analysed"),
            ("measured_at", "2026-01-01"),
            ("instrument_id", &instrument_id.to_string()),
            ("capacity_mah", "1900"),
            ("discharge_ma", "500"),
        ]),
    );

    assert!(matches!(result, CommandResult::Redirect { .. }), "a valid reading should redirect, not re-render with an error");
    let measurements = state.db.lock().unwrap().measurements(1).unwrap();
    assert_eq!(measurements.len(), 1);
    assert_eq!(measurements[0].capacity_mah, Some(1900));
}

// ── Batch add reading keeps every row's values on a validation error ────

#[test]
fn batch_measure_preserves_row_values_on_validation_error() {
    let (_dir, state) = fresh_state();
    let instrument_id;
    {
        let db = state.db.lock().unwrap();
        db.create(1, "AA", Some(2000), "", "", "").unwrap();
        instrument_id = db.add_instrument("Charger", 2).unwrap().id;
        db.add_mode(instrument_id, "Analyse", "analysed", "", None).unwrap();
    }

    // Capacity with no discharge current is refused by clean_reading().
    let html = expect_page(router::submit_form(
        &state,
        "/batch/measure",
        &fields(&[
            ("kind", "analysed"),
            ("measured_at", "2026-01-01"),
            ("instrument_id", &instrument_id.to_string()),
            ("battery_id_1", "1"),
            ("capacity_mah_1", "2000"),
            ("notes_1", "keep me"),
        ]),
    ));

    assert!(html.contains("discharge current"), "should surface the clean_reading error");
    assert!(html.contains("keep me"), "row values should survive the error instead of resetting");
}

// ── List filters/sort persist to a later, unrelated navigation ──────────

#[test]
fn list_filters_persist_to_a_later_bare_navigation() {
    let (_dir, state) = fresh_state();
    {
        let db = state.db.lock().unwrap();
        db.create(1, "AA", Some(2000), "", "", "").unwrap();
        db.create(2, "AAA", Some(1000), "", "", "").unwrap();
    }

    // Set filters/sort explicitly, as the Filter button / sort dropdown would.
    router::render_page(&state, "/", "type=AA&sort=location");

    // A later bare "/" (e.g. clicking the header brand link) carries no query at all.
    let html = expect_page(router::render_page(&state, "/", ""));

    assert!(html.contains(r#"value="AA" selected"#), "type filter should still be AA");
    assert!(html.contains(r#"value="location" selected"#), "sort should still be Location");
}

#[test]
fn an_explicit_empty_type_clears_the_persisted_filter() {
    let (_dir, state) = fresh_state();
    state.db.lock().unwrap().create(1, "AA", Some(2000), "", "", "").unwrap();

    router::render_page(&state, "/", "type=AA");
    // Choosing "Any type" submits an explicit empty value, not an absent key —
    // that has to actually clear the persisted filter, not be ignored as "no change".
    let html = expect_page(router::render_page(&state, "/", "type="));

    assert!(!html.contains(r#"value="AA" selected"#), "the AA filter should be cleared, not still persisted");
}

// ── Procedure reorder: swaps order, doesn't reopen an edit form ─────────

#[test]
fn moving_a_procedure_down_swaps_its_order_and_does_not_reopen_edit() {
    let (_dir, state) = fresh_state();
    let (instrument_id, first_id, second_id);
    {
        let db = state.db.lock().unwrap();
        instrument_id = db.add_instrument("Charger", 1).unwrap().id;
        first_id = db.add_mode(instrument_id, "Charge", "charged", "", None).unwrap().id;
        second_id = db.add_mode(instrument_id, "Analyse", "analysed", "", None).unwrap().id;
    }

    let result = router::submit_form(
        &state,
        &format!("/instruments/{instrument_id}/procedures/{first_id}/move"),
        &fields(&[("direction", "down")]),
    );

    let (path, _query) = expect_redirect(result);
    // Should scroll back to the moved card via a fragment, not reopen its
    // edit form via the old "?open={id}" mechanism (removed — the reorder
    // buttons live in the always-visible header row now, not behind Edit).
    assert_eq!(path, format!("/instruments#procedure-{first_id}"));

    let modes = state.db.lock().unwrap().list_instruments().unwrap()[0].modes.clone();
    assert_eq!(modes[0].id, second_id, "Analyse should now be first");
    assert_eq!(modes[1].id, first_id, "Charge should now be second");
}

// ── PDF output mode: every print action redirects with save_pdf_ids ─────
// instead of touching the network printer, so `dist/app.js` can call
// `save_labels_pdf` (which needs a real AppHandle, unavailable in plain
// router functions) once the redirect lands. See `label_output_is_pdf`.

#[test]
fn reprinting_one_battery_in_pdf_mode_redirects_with_its_id_instead_of_printing() {
    let (_dir, state) = fresh_state();
    {
        let db = state.db.lock().unwrap();
        db.create(1, "AA", Some(2000), "", "", "").unwrap();
        db.set_setting("label_output", "pdf").unwrap();
    }

    let (path, query) = expect_redirect(router::submit_form(&state, "/b/1/print", &fields(&[])));

    assert_eq!(path, "/b/1");
    let query = query.unwrap_or_default();
    assert!(query.contains("save_pdf_ids=1"), "should carry the id to save as a PDF: {query:?}");
    assert!(!query.contains("msg="), "should not claim a label was printed");
}

#[test]
fn batch_print_in_pdf_mode_redirects_with_a_csv_of_ids_instead_of_printing() {
    let (_dir, state) = fresh_state();
    {
        let db = state.db.lock().unwrap();
        db.create(1, "AA", Some(2000), "", "", "").unwrap();
        db.create(2, "AA", Some(2000), "", "", "").unwrap();
        db.set_setting("label_output", "pdf").unwrap();
    }

    let (path, query) = expect_redirect(router::submit_form(
        &state,
        "/print",
        &fields(&[("spec", "1-2"), ("confirm", "yes")]),
    ));

    assert_eq!(path, "/print");
    let query = query.unwrap_or_default();
    assert!(query.contains("save_pdf_ids=1%2C2") || query.contains("save_pdf_ids=1,2"), "should carry both ids: {query:?}");
}

#[test]
fn adding_a_battery_with_print_in_pdf_mode_redirects_with_its_id_instead_of_printing() {
    let (_dir, state) = fresh_state();
    state.db.lock().unwrap().set_setting("label_output", "pdf").unwrap();

    let (path, query) = expect_redirect(router::submit_form(
        &state,
        "/add",
        &fields(&[("type", "AA"), ("nominal_mah", "2000"), ("count", "1"), ("print_label", "on")]),
    ));

    assert!(path.starts_with("/b/"), "should still redirect to the new battery's page: {path:?}");
    let query = query.unwrap_or_default();
    assert!(query.contains("save_pdf_ids="), "should carry the new battery's id to save as a PDF: {query:?}");
}

#[test]
fn adding_a_battery_without_print_in_pdf_mode_does_not_mention_save_pdf_ids() {
    let (_dir, state) = fresh_state();
    state.db.lock().unwrap().set_setting("label_output", "pdf").unwrap();

    let (_path, query) = expect_redirect(router::submit_form(
        &state,
        "/add",
        &fields(&[("type", "AA"), ("nominal_mah", "2000"), ("count", "1")]),
    ));

    let query = query.unwrap_or_default();
    assert!(!query.contains("save_pdf_ids"), "print wasn't requested, so no PDF should be triggered: {query:?}");
}
