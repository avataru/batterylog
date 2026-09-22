//! `#[tauri::command]` entry points called via `invoke()` from `dist/app.js`
//! and `dist/scanner.js`. Page navigation/forms go through the generic
//! `render_page`/`submit_form` router; the scan flow's four JSON actions and
//! the label PNG download get their own typed commands.

use std::collections::HashMap;

use batteries_core::{config, labels, pdf};
use serde::Serialize;
use serde_json::Value;
use tauri::State;

use crate::router::{self, CommandResult};
use crate::state::AppState;

#[tauri::command]
pub fn render_page(state: State<AppState>, path: String, query: String) -> CommandResult {
    router::render_page(&state, &path, &query)
}

#[tauri::command]
pub fn app_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Backs every destructive-action confirmation in the UI. A JS
/// `window.confirm()` inside the webview never actually surfaces a dialog in
/// this app (silently resolves without prompting), so every "are you sure"
/// goes through this native message box instead, which is the same
/// mechanism already proven reliable for the file save/open dialogs.
#[tauri::command]
pub fn confirm_dialog(app: tauri::AppHandle, message: String) -> bool {
    use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
    app.dialog()
        .message(message)
        .title("Confirm")
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::OkCancelCustom("Continue".into(), "Cancel".into()))
        .blocking_show()
}

#[tauri::command]
pub fn submit_form(state: State<AppState>, path: String, fields: HashMap<String, String>) -> CommandResult {
    router::submit_form(&state, &path, &fields)
}

#[derive(Serialize)]
pub struct ResolvedBattery {
    #[serde(flatten)]
    battery: Value,
    deleted: bool,
}

#[tauri::command]
pub fn resolve_code(state: State<AppState>, code: String) -> Result<ResolvedBattery, String> {
    let id = labels::decode_payload(&code)?;
    let db = state.db.lock().unwrap();
    let battery = db.get(id).map_err(|e| e.to_string())?.ok_or_else(|| format!("No battery has ever had id {id}."))?;
    let deleted = battery.deleted_at.is_some();
    Ok(ResolvedBattery { battery: serde_json::to_value(battery).unwrap(), deleted })
}

#[tauri::command]
pub fn restore_battery_api(state: State<AppState>, id: i64) -> Result<Value, String> {
    let db = state.db.lock().unwrap();
    let battery = db.restore(id).map_err(|e| e.to_string())?.ok_or_else(|| "No such battery.".to_string())?;
    Ok(serde_json::to_value(battery).unwrap())
}

#[tauri::command]
pub fn set_location_api(state: State<AppState>, id: i64, location: String) -> Result<Value, String> {
    let db = state.db.lock().unwrap();
    let battery = db.set_location(id, &location).map_err(|e| e.to_string())?.ok_or_else(|| "No such battery.".to_string())?;
    Ok(serde_json::to_value(battery).unwrap())
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn add_measurement_api(
    state: State<AppState>,
    id: i64,
    kind: String,
    measured_at: String,
    capacity_mah: String,
    ir_mohm: String,
    instrument_id: String,
    mode_id: String,
    discharge_ma: String,
    charge_ma: String,
    notes: String,
) -> Result<Value, String> {
    let db = state.db.lock().unwrap();
    let parse = |s: &str| -> Option<i64> { let s = s.trim(); if s.is_empty() { None } else { s.parse().ok() } };
    let capacity_mah = crate::helpers::unsigned_number(&capacity_mah, "Capacity")?;
    let ir_mohm = crate::helpers::unsigned_number(&ir_mohm, "Resistance")?;
    let discharge_ma = crate::helpers::unsigned_number(&discharge_ma, "Discharge current")?;
    let charge_ma = crate::helpers::unsigned_number(&charge_ma, "Charge current")?;
    let (instrument_id, mode_id) = (parse(&instrument_id), parse(&mode_id));
    crate::helpers::clean_reading(&db, &kind, capacity_mah, ir_mohm, instrument_id, mode_id, discharge_ma, charge_ma)?;
    let measurement = db
        .add_measurement(id, &kind, &measured_at, capacity_mah, ir_mohm, instrument_id, mode_id, discharge_ma, charge_ma, &notes)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "No such battery.".to_string())?;
    Ok(serde_json::to_value(measurement).unwrap())
}

#[tauri::command]
pub fn get_label_png(state: State<AppState>, id: i64) -> Result<Vec<u8>, String> {
    let db = state.db.lock().unwrap();
    let battery = db.get(id).map_err(|e| e.to_string())?.ok_or_else(|| format!("No battery has id {id}."))?;
    let text = config::get(&db, "tape_mm").as_text();
    let mm: f64 = text.parse().unwrap_or(9.0);
    let id_text_scale = config::get(&db, "id_text_scale").as_f64();
    let raster = labels::render_label(battery.id, (mm * 10.0).round() as i64, id_text_scale)?;
    raster.to_png_bytes()
}

/// Opens a native file picker for an existing `batteries.db`. Returns
/// `None` if the user cancelled. Split from
/// [`import_database`] so the JS side can show the user which file they
/// picked in a confirmation prompt before anything destructive happens.
#[tauri::command]
pub fn pick_database_file(app: tauri::AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let picked = app
        .dialog()
        .file()
        .add_filter("SQLite database", &["db"])
        .blocking_pick_file();
    let Some(file_path) = picked else { return Ok(None) };
    let source = file_path.into_path().map_err(|e| e.to_string())?;
    Ok(Some(source.display().to_string()))
}

/// Replaces this app's database with `source`, live — no restart needed.
#[tauri::command]
pub fn import_database(state: State<AppState>, source: String) -> Result<(), String> {
    let mut db = state.db.lock().unwrap();
    db.import_and_reopen(std::path::Path::new(&source))
}

/// Opens a native save dialog and writes the battery's label PNG there.
/// Returns `None` if the user cancelled.
#[tauri::command]
pub fn save_label_png(app: tauri::AppHandle, state: State<AppState>, id: i64) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let bytes = get_label_png(state, id)?;
    let picked = app
        .dialog()
        .file()
        .add_filter("PNG image", &["png"])
        .set_file_name(&format!("battery-{id:03}.png"))
        .blocking_save_file();
    let Some(file_path) = picked else { return Ok(None) };
    let destination = file_path.into_path().map_err(|e| e.to_string())?;
    std::fs::write(&destination, bytes).map_err(|e| e.to_string())?;
    Ok(Some(destination.display().to_string()))
}

/// Opens a native save dialog and writes a PDF containing a label for every
/// id in `ids` — one page per label, or several tiled per page, per the
/// `pdf_layout` setting. This is what every "print" action in the app calls
/// instead of `labels::print_labels` when `label_output` is "pdf" (see
/// `router::label_output_is_pdf`) — those actions redirect back with the
/// relevant ids in a `save_pdf_ids` query field, and `app.js` calls this
/// command directly once the redirect lands, since opening a save dialog
/// needs the `AppHandle` only a real command has. Returns `None` if the
/// user cancelled.
#[tauri::command]
pub fn save_labels_pdf(app: tauri::AppHandle, state: State<AppState>, ids: Vec<i64>) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    if ids.is_empty() {
        return Err("No battery ids were given.".to_string());
    }
    let db = state.db.lock().unwrap();
    let tape_mm: f64 = config::get(&db, "tape_mm").as_text().parse().unwrap_or(9.0);
    let tape_mm_x10 = (tape_mm * 10.0).round() as i64;
    let id_text_scale = config::get(&db, "id_text_scale").as_f64();
    let layout = if config::get(&db, "pdf_layout").as_text() == "grid" {
        pdf::PdfLayout::Grid
    } else {
        pdf::PdfLayout::Pages
    };
    let rasters: Vec<_> = ids
        .iter()
        .map(|&id| labels::render_label(id, tape_mm_x10, id_text_scale))
        .collect::<Result<_, _>>()?;
    drop(db);

    let bytes = pdf::render_labels_pdf(&rasters, layout);
    let default_name =
        if let [only_id] = ids[..] { format!("battery-{only_id:03}.pdf") } else { "labels.pdf".to_string() };
    let picked = app
        .dialog()
        .file()
        .add_filter("PDF document", &["pdf"])
        .set_file_name(&default_name)
        .blocking_save_file();
    let Some(file_path) = picked else { return Ok(None) };
    let destination = file_path.into_path().map_err(|e| e.to_string())?;
    std::fs::write(&destination, bytes).map_err(|e| e.to_string())?;
    Ok(Some(destination.display().to_string()))
}

/// Opens a native save dialog and writes a consistent copy of the live
/// database there via SQLite's own backup API (safe to do while the app is
/// running). Returns `None` if the user cancelled.
#[tauri::command]
pub fn export_database(app: tauri::AppHandle, state: State<AppState>) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let picked = app
        .dialog()
        .file()
        .add_filter("SQLite database", &["db"])
        .set_file_name("batteries.db")
        .blocking_save_file();
    let Some(file_path) = picked else { return Ok(None) };
    let destination = file_path.into_path().map_err(|e| e.to_string())?;
    let db = state.db.lock().unwrap();
    let written = db.backup(&destination).map_err(|e| e.to_string())?;
    Ok(Some(written.display().to_string()))
}
