pub mod commands;
pub mod helpers;
pub mod icons;
pub mod router;
pub mod state;
pub mod templates;
pub mod view;

use batteries_core::db::Db;
use state::AppState;
use tauri::{Manager, PhysicalPosition, PhysicalSize};
use tauri_plugin_window_state::{AppHandleExt, StateFlags};

/// Where the window goes and how big to make its inner area.
struct Placement {
    x: i32,
    y: i32,
    inner_width: u32,
    inner_height: u32,
}

/// Half the work area's width and all of its height, centred horizontally.
/// `frame_*` is the window's decoration (outer size minus inner size): a
/// window is resized by its inner size, so the frame has to come off to make
/// the *outer* size fill the work area.
fn placement(area_x: i32, area_y: i32, area_width: u32, area_height: u32, frame_width: u32, frame_height: u32) -> Placement {
    let width = area_width / 2;
    Placement {
        x: area_x + ((area_width - width) / 2) as i32,
        y: area_y,
        inner_width: width.saturating_sub(frame_width),
        inner_height: area_height.saturating_sub(frame_height),
    }
}

/// Whether the window-state plugin has a file from an earlier run, i.e.
/// whether it already put the window back where it was left.
fn has_saved_window_state(app: &tauri::AppHandle) -> bool {
    app.path().app_config_dir().map(|dir| dir.join(app.filename()).exists()).unwrap_or(false)
}

/// `tauri.conf.json` can only give a fixed size, so on a first run the window
/// opens at half the width and the full height of the screen it lands on (the
/// work area, so the taskbar stays clear) here instead. Keeps the configured
/// size if the monitor can't be read.
fn fit_to_screen(window: &tauri::WebviewWindow) {
    let monitor = match window.current_monitor() {
        Ok(Some(monitor)) => monitor,
        _ => match window.primary_monitor() {
            Ok(Some(monitor)) => monitor,
            _ => return,
        },
    };
    let (Ok(outer), Ok(inner)) = (window.outer_size(), window.inner_size()) else { return };
    let area = monitor.work_area();
    let put = placement(
        area.position.x,
        area.position.y,
        area.size.width,
        area.size.height,
        outer.width.saturating_sub(inner.width),
        outer.height.saturating_sub(inner.height),
    );
    let _ = window.set_size(PhysicalSize::new(put.inner_width, put.inner_height));
    let _ = window.set_position(PhysicalPosition::new(put.x, put.y));
}

/// Setting this moves the database and the saved window state out of
/// `%APPDATA%\com.batteries.log`, so a test instance never touches real data.
/// (`%APPDATA%` itself can't be redirected: Tauri asks Windows for that folder
/// rather than reading the variable.)
const DATA_DIR_ENV: &str = "BATTERY_LOG_DATA_DIR";

pub fn run() {
    let data_dir_override = std::env::var_os(DATA_DIR_ENV).filter(|v| !v.is_empty()).map(std::path::PathBuf::from);

    // Puts the window back at its last size, position and maximised state as
    // it's created, and saves them on exit.
    let mut window_state = tauri_plugin_window_state::Builder::default()
        .with_state_flags(StateFlags::SIZE | StateFlags::POSITION | StateFlags::MAXIMIZED);
    if let Some(dir) = &data_dir_override {
        // The plugin joins this onto the config dir, and joining an absolute
        // path replaces the base — which is what moves the file.
        window_state = window_state.with_filename(dir.join(tauri_plugin_window_state::DEFAULT_FILENAME).to_string_lossy());
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(window_state.build())
        .setup(move |app| {
            let data_dir = match data_dir_override {
                Some(dir) => dir,
                None => app.path().app_data_dir().expect("app data dir available"),
            };
            std::fs::create_dir_all(&data_dir).ok();
            let db_path = data_dir.join("batteries.db");
            let db = Db::open(&db_path).expect("failed to open the database");
            let tera = templates::build();
            app.manage(AppState { db: std::sync::Mutex::new(db), tera });

            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_title(&format!("Battery Log v{}", env!("CARGO_PKG_VERSION")));
                if !has_saved_window_state(app.handle()) {
                    fit_to_screen(&window);
                }
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::render_page,
            commands::submit_form,
            commands::confirm_dialog,
            commands::app_version,
            commands::resolve_code,
            commands::restore_battery_api,
            commands::set_location_api,
            commands::add_measurement_api,
            commands::get_label_png,
            commands::save_label_png,
            commands::save_labels_pdf,
            commands::pick_database_file,
            commands::import_database,
            commands::export_database,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Battery Log");
}

#[cfg(test)]
mod tests {
    use super::placement;

    #[test]
    fn opens_at_half_the_width_and_all_the_height_of_the_work_area() {
        // A 3440x1440 ultrawide with a 48px taskbar, and a window whose
        // frame is 16 wide and 39 tall.
        let put = placement(0, 0, 3440, 1392, 16, 39);

        assert_eq!(put.inner_width + 16, 1720, "outer width should be half the work area");
        assert_eq!(put.inner_height + 39, 1392, "outer height should be the whole work area");
        assert_eq!(put.x, 860, "centred: a quarter of the width free on each side");
        assert_eq!(put.y, 0);
    }

    #[test]
    fn follows_a_work_area_that_does_not_start_at_the_origin() {
        // A second monitor to the right of a 1920-wide one.
        let put = placement(1920, 0, 1920, 1040, 16, 39);

        assert_eq!(put.x, 1920 + 480);
        assert_eq!(put.inner_width + 16, 960);
    }
}
