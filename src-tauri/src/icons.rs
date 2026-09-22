//! Inlined nav/action icon SVGs, exposed to templates as an `icon()` Tera
//! global. Embedding via `include_str!` avoids any runtime path resolution
//! across install locations/platforms.

pub fn svg(name: &str) -> &'static str {
    match name {
        "add" => include_str!("../icons_src/add.svg"),
        "alert" => include_str!("../icons_src/alert.svg"),
        "batch-location" => include_str!("../icons_src/batch-location.svg"),
        "batch-measure" => include_str!("../icons_src/batch-measure.svg"),
        "delete" => include_str!("../icons_src/delete.svg"),
        "edit" => include_str!("../icons_src/edit.svg"),
        "filter" => include_str!("../icons_src/filter.svg"),
        "gauge" => include_str!("../icons_src/gauge.svg"),
        "grid" => include_str!("../icons_src/grid.svg"),
        "info" => include_str!("../icons_src/info.svg"),
        "move-down" => include_str!("../icons_src/move-down.svg"),
        "move-up" => include_str!("../icons_src/move-up.svg"),
        "print" => include_str!("../icons_src/print.svg"),
        "restore" => include_str!("../icons_src/restore.svg"),
        "scan" => include_str!("../icons_src/scan.svg"),
        "settings" => include_str!("../icons_src/settings.svg"),
        "table" => include_str!("../icons_src/table.svg"),
        _ => "",
    }
}
