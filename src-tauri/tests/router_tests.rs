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

// ── Battery matching ─────────────────────────────────────────────────────

fn seed_pool_battery(state: &AppState, id: i64, brand: &str, capacity_mah: i64, ir_mohm: Option<i64>) {
    let db = state.db.lock().unwrap();
    db.create(id, "AA", Some(2000), brand, "storage", "").unwrap();
    db.add_measurement(id, "analysed", "2026-01-01", Some(capacity_mah), ir_mohm, None, None, Some(500), None, "")
        .unwrap();
}

#[test]
fn match_page_renders_with_no_pool_batteries() {
    let (_dir, state) = fresh_state();
    let html = expect_page(router::render_page(&state, "/match", ""));
    assert!(html.contains("Match batteries"));
    assert!(html.contains("storage"), "should name the default pool location: {html}");
}

#[test]
fn match_preview_suggests_the_weakest_cells_for_a_low_draw_request() {
    let (_dir, state) = fresh_state();
    seed_pool_battery(&state, 1, "Eneloop", 1900, Some(80));
    seed_pool_battery(&state, 2, "Eneloop", 1200, Some(200));
    seed_pool_battery(&state, 3, "Eneloop", 1950, Some(70));

    let html = expect_page(router::submit_form(
        &state,
        "/match",
        &fields(&[("type", "AA"), ("count", "1"), ("draw", "low"), ("location", "Torch")]),
    ));

    assert!(html.contains(">002<"), "the weakest cell (002) should be suggested: {html}");
    // Nothing should have moved yet — this is only a preview.
    assert_eq!(state.db.lock().unwrap().get(2).unwrap().unwrap().location, "storage");
}

#[test]
fn match_confirm_moves_the_suggested_batteries_and_logs_the_match() {
    let (_dir, state) = fresh_state();
    seed_pool_battery(&state, 1, "Eneloop", 1900, Some(80));
    seed_pool_battery(&state, 2, "Eneloop", 1850, Some(90));
    seed_pool_battery(&state, 3, "Duracell", 1950, Some(70));

    let (path, query) = expect_redirect(router::submit_form(
        &state,
        "/match",
        &fields(&[
            ("type", "AA"), ("count", "2"), ("draw", "low"), ("location", "Torch"), ("confirm", "yes"),
        ]),
    ));
    assert_eq!(path, "/match");
    assert!(query.unwrap_or_default().contains("msg="));

    // The single-brand (Eneloop) bucket is sufficient, so 1 and 2 (not the
    // Duracell) should have moved.
    let db = state.db.lock().unwrap();
    assert_eq!(db.get(1).unwrap().unwrap().location, "Torch");
    assert_eq!(db.get(2).unwrap().unwrap().location, "Torch");
    assert_eq!(db.get(3).unwrap().unwrap().location, "storage");

    let matches = db.list_matches().unwrap();
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].location, "Torch");
    assert_eq!(matches[0].requested_type, "AA");
    assert_eq!(matches[0].draw, "low");
    assert!(matches[0].returned_at.is_none());
    let mut ids = matches[0].battery_ids.clone();
    ids.sort();
    assert_eq!(ids, vec![1, 2]);
}

#[test]
fn match_confirm_reports_an_error_instead_of_moving_anything_when_the_pool_is_short() {
    let (_dir, state) = fresh_state();
    seed_pool_battery(&state, 1, "Eneloop", 1900, Some(80));

    let html = expect_page(router::submit_form(
        &state,
        "/match",
        &fields(&[("type", "AA"), ("count", "3"), ("draw", "low"), ("location", "Torch")]),
    ));

    assert!(html.contains("Only 1"), "should explain the shortfall: {html}");
    assert_eq!(state.db.lock().unwrap().list_matches().unwrap().len(), 0);
}

#[test]
fn a_matched_battery_returning_to_storage_by_any_path_closes_out_its_match() {
    let (_dir, state) = fresh_state();
    seed_pool_battery(&state, 1, "Eneloop", 1900, Some(80));

    router::submit_form(
        &state,
        "/match",
        &fields(&[
            ("type", "AA"), ("count", "1"), ("draw", "low"), ("location", "Torch"), ("confirm", "yes"),
        ]),
    );
    let match_id = state.db.lock().unwrap().list_matches().unwrap()[0].id;

    // Moved via the single-battery location form, not a dedicated "return"
    // action — the close-out has to fire from every location-change path.
    router::submit_form(&state, "/b/1/location", &fields(&[("location", "Storage")]));

    let matches = state.db.lock().unwrap().list_matches().unwrap();
    assert_eq!(matches[0].id, match_id);
    assert!(matches[0].returned_at.is_some(), "should be closed out once battery 1 is back in the pool");
}

fn match_three_to_torch(state: &AppState) {
    for id in 1..=3 {
        seed_pool_battery(state, id, "Eneloop", 1900, Some(80));
    }
    router::submit_form(
        state,
        "/match",
        &fields(&[
            ("type", "AA"), ("count", "3"), ("draw", "low"), ("location", "Torch"), ("confirm", "yes"),
        ]),
    );
}

#[test]
fn returning_one_battery_of_a_match_returns_the_whole_set() {
    let (_dir, state) = fresh_state();
    match_three_to_torch(&state);

    let (_, query) = expect_redirect(router::submit_form(&state, "/b/2/location", &fields(&[("location", "storage")])));

    let query = query.unwrap_or_default();
    assert!(query.contains("001") && query.contains("003"), "should say which batteries came back too: {query:?}");
    let db = state.db.lock().unwrap();
    for id in 1..=3 {
        assert_eq!(db.get(id).unwrap().unwrap().location, "storage", "battery {id} should be back in the pool");
        assert_eq!(db.history(id).unwrap()[0].location, "storage", "battery {id} should get its own history entry");
    }
    assert!(db.list_matches().unwrap()[0].returned_at.is_some());
}

#[test]
fn returning_a_whole_set_by_batch_does_not_double_report_it() {
    let (_dir, state) = fresh_state();
    match_three_to_torch(&state);

    let (_, query) = expect_redirect(router::submit_form(
        &state,
        "/batch/location",
        &fields(&[("spec", "1-3"), ("location", "storage")]),
    ));

    let query = query.unwrap_or_default();
    assert!(!query.contains("Also+returned") && !query.contains("Also%20returned"), "every battery was in the batch: {query:?}");
    let db = state.db.lock().unwrap();
    for id in 1..=3 {
        assert_eq!(db.get(id).unwrap().unwrap().location, "storage");
        let history = db.history(id).unwrap();
        assert_eq!(history.iter().filter(|h| h.location == "storage").count(), 2, "one original + one return, no duplicate entry");
    }
}

#[test]
fn a_battery_already_moved_elsewhere_still_returns_with_its_set() {
    let (_dir, state) = fresh_state();
    match_three_to_torch(&state);
    router::submit_form(&state, "/b/3/location", &fields(&[("location", "Drawer")]));

    router::submit_form(&state, "/b/1/location", &fields(&[("location", "storage")]));

    let db = state.db.lock().unwrap();
    assert_eq!(db.get(3).unwrap().unwrap().location, "storage");
}

#[test]
fn return_button_moves_the_whole_set_back_and_closes_the_match() {
    let (_dir, state) = fresh_state();
    match_three_to_torch(&state);
    router::submit_form(&state, "/b/3/location", &fields(&[("location", "Drawer")]));
    let match_id = state.db.lock().unwrap().list_matches().unwrap()[0].id;

    let (path, query) = expect_redirect(router::submit_form(&state, &format!("/match/{match_id}/return"), &fields(&[])));

    assert_eq!(path, "/match/log");
    assert!(query.unwrap_or_default().contains("001%2C+002%2C+003"), "should list every battery returned");
    let db = state.db.lock().unwrap();
    for id in 1..=3 {
        assert_eq!(db.get(id).unwrap().unwrap().location, "storage");
    }
    assert!(db.list_matches().unwrap()[0].returned_at.is_some());
}

#[test]
fn match_log_shows_an_in_use_pill_and_return_button_only_for_open_matches() {
    let (_dir, state) = fresh_state();
    match_three_to_torch(&state);

    let open = expect_page(router::render_page(&state, "/match/log", ""));
    assert!(open.contains("In use in <strong>Torch</strong>"), "{open}");
    assert!(open.contains("/return\""), "an open match should offer Return");
    assert!(
        !open.contains("<title>"),
        "an icon's own <title> overrides the button's tooltip, so none should be embedded"
    );

    let match_id = state.db.lock().unwrap().list_matches().unwrap()[0].id;
    router::submit_form(&state, &format!("/match/{match_id}/return"), &fields(&[]));

    let closed = expect_page(router::render_page(&state, "/match/log", ""));
    assert!(!closed.contains("In use in"));
    assert!(!closed.contains("/return\""), "a returned match has nothing left to return");
}

#[test]
fn match_log_page_renders_past_matches() {
    let (_dir, state) = fresh_state();
    seed_pool_battery(&state, 1, "Eneloop", 1900, Some(80));
    router::submit_form(
        &state,
        "/match",
        &fields(&[
            ("type", "AA"), ("count", "1"), ("draw", "low"), ("location", "Torch"), ("confirm", "yes"),
        ]),
    );

    let html = expect_page(router::render_page(&state, "/match/log", ""));
    assert!(html.contains("Torch"));
    assert!(html.contains(">001<"));
}

#[test]
fn deleting_a_match_leaves_its_batteries_where_they_are() {
    let (_dir, state) = fresh_state();
    seed_pool_battery(&state, 1, "Eneloop", 1900, Some(80));
    router::submit_form(
        &state,
        "/match",
        &fields(&[
            ("type", "AA"), ("count", "1"), ("draw", "low"), ("location", "Torch"), ("confirm", "yes"),
        ]),
    );
    let match_id = state.db.lock().unwrap().list_matches().unwrap()[0].id;

    let (path, _) = expect_redirect(router::submit_form(&state, &format!("/match/{match_id}/delete"), &fields(&[])));

    assert_eq!(path, "/match/log");
    let db = state.db.lock().unwrap();
    assert!(db.list_matches().unwrap().is_empty());
    assert_eq!(db.get(1).unwrap().unwrap().location, "Torch", "no cascade: the battery stays put");
    assert_eq!(db.history(1).unwrap().len(), 2, "no cascade: location history is untouched");
}

#[test]
fn deleting_a_location_entry_keeps_the_current_location_and_any_match() {
    let (_dir, state) = fresh_state();
    seed_pool_battery(&state, 1, "Eneloop", 1900, Some(80));
    router::submit_form(
        &state,
        "/match",
        &fields(&[
            ("type", "AA"), ("count", "1"), ("draw", "low"), ("location", "Torch"), ("confirm", "yes"),
        ]),
    );
    let newest = state.db.lock().unwrap().history(1).unwrap()[0].clone();
    assert_eq!(newest.location, "Torch");

    let (path, _) = expect_redirect(router::submit_form(&state, &format!("/history/{}/delete", newest.id), &fields(&[])));

    assert_eq!(path, "/b/1");
    let db = state.db.lock().unwrap();
    let history = db.history(1).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].location, "storage");
    assert_eq!(db.get(1).unwrap().unwrap().location, "Torch", "current location is not rewound");
    assert_eq!(db.list_matches().unwrap().len(), 1, "no cascade: the match stays in the log");
}

#[test]
fn battery_page_and_match_log_offer_delete_buttons() {
    let (_dir, state) = fresh_state();
    seed_pool_battery(&state, 1, "Eneloop", 1900, Some(80));
    router::submit_form(
        &state,
        "/match",
        &fields(&[
            ("type", "AA"), ("count", "1"), ("draw", "low"), ("location", "Torch"), ("confirm", "yes"),
        ]),
    );

    let detail = expect_page(router::render_page(&state, "/b/1", ""));
    assert!(detail.contains("/history/") && detail.contains("Delete this location entry"));
    let log = expect_page(router::render_page(&state, "/match/log", ""));
    assert!(log.contains("/delete") && log.contains("Delete this match"));
}
