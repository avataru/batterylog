//! SQLite storage for the battery inventory.

use std::path::Path;

use chrono::Utc;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

pub type DbResult<T> = rusqlite::Result<T>;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS batteries (
    id           INTEGER PRIMARY KEY,
    type         TEXT    NOT NULL,
    nominal_mah  INTEGER,
    brand        TEXT    NOT NULL DEFAULT '',
    location     TEXT    NOT NULL DEFAULT '',
    notes        TEXT    NOT NULL DEFAULT '',
    created_at   TEXT    NOT NULL,
    updated_at   TEXT    NOT NULL,
    deleted_at   TEXT
);

CREATE TABLE IF NOT EXISTS settings (
    key        TEXT PRIMARY KEY,
    value      TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS location_history (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    battery_id  INTEGER NOT NULL REFERENCES batteries(id) ON DELETE CASCADE,
    location    TEXT    NOT NULL,
    moved_at    TEXT    NOT NULL
);

CREATE TABLE IF NOT EXISTS instruments (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    name       TEXT    NOT NULL,
    slots      INTEGER NOT NULL DEFAULT 1,
    created_at TEXT    NOT NULL,
    updated_at TEXT    NOT NULL
);

CREATE TABLE IF NOT EXISTS instrument_modes (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    instrument_id  INTEGER NOT NULL REFERENCES instruments(id) ON DELETE CASCADE,
    name           TEXT    NOT NULL,
    kind           TEXT    NOT NULL DEFAULT 'analysed',
    details        TEXT    NOT NULL DEFAULT '',
    position       INTEGER NOT NULL DEFAULT 0,
    created_at     TEXT    NOT NULL,
    updated_at     TEXT    NOT NULL,
    fields         TEXT    NOT NULL DEFAULT '{}'
);

CREATE TABLE IF NOT EXISTS measurements (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    battery_id    INTEGER NOT NULL REFERENCES batteries(id) ON DELETE CASCADE,
    kind          TEXT    NOT NULL,
    measured_at   TEXT    NOT NULL,
    capacity_mah  INTEGER,
    ir_mohm       INTEGER,
    instrument_id INTEGER REFERENCES instruments(id) ON DELETE SET NULL,
    mode_id       INTEGER REFERENCES instrument_modes(id) ON DELETE SET NULL,
    discharge_ma  INTEGER,
    charge_ma     INTEGER,
    notes         TEXT    NOT NULL DEFAULT '',
    created_at    TEXT    NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_history_battery ON location_history(battery_id);
CREATE INDEX IF NOT EXISTS idx_batteries_deleted ON batteries(deleted_at);
CREATE INDEX IF NOT EXISTS idx_measurements_battery
    ON measurements(battery_id, measured_at);
CREATE INDEX IF NOT EXISTS idx_modes_instrument ON instrument_modes(instrument_id);
";

fn now() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Open a connection at `path` with foreign keys on and WAL mode enabled.
/// `init` (WAL mode + schema) should be called once per database file, not
/// per connection — callers typically hold one [`Db`] wrapping one open
/// connection for the app's lifetime instead of reconnecting per call.
pub struct Db {
    conn: Connection,
    path: std::path::PathBuf,
}

fn open_at(path: &Path) -> DbResult<Connection> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "busy_timeout", 5000)?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.execute_batch(SCHEMA)?;
    Ok(conn)
}

impl Db {
    pub fn open(path: &Path) -> DbResult<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let conn = open_at(path)?;
        Ok(Db { conn, path: path.to_path_buf() })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Replace the app's database with `source` (e.g. a `batteries.db`
    /// backup) and reopen the live connection against it, in place — no app
    /// restart needed. Only the *current* schema shape is supported; this
    /// does not replay historical migrations, so `source` must already be
    /// at the schema this version expects.
    ///
    /// The live connection is dropped and replaced with an in-memory one
    /// *before* touching the file on disk: on Windows, a file that's still
    /// open by this process's own connection can't be overwritten.
    pub fn import_and_reopen(&mut self, source: &Path) -> Result<(), String> {
        let path = self.path.clone();
        self.conn = Connection::open_in_memory().map_err(|e| e.to_string())?;

        for suffix in ["-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
        std::fs::copy(source, &path).map_err(|e| e.to_string())?;

        self.conn = open_at(&path).map_err(|e| e.to_string())?;
        Ok(())
    }
}

// ── Row types ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Battery {
    pub id: i64,
    pub r#type: String,
    pub nominal_mah: Option<i64>,
    pub brand: String,
    pub location: String,
    pub notes: String,
    pub created_at: String,
    pub updated_at: String,
    pub deleted_at: Option<String>,
    /// Only set by [`soft_delete`]; not a DB column.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hard_deleted: Option<bool>,
}

fn battery_from_row(row: &Row) -> DbResult<Battery> {
    Ok(Battery {
        id: row.get("id")?,
        r#type: row.get("type")?,
        nominal_mah: row.get("nominal_mah")?,
        brand: row.get("brand")?,
        location: row.get("location")?,
        notes: row.get("notes")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
        deleted_at: row.get("deleted_at")?,
        hard_deleted: None,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocationEntry {
    pub location: String,
    pub moved_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Measurement {
    pub id: i64,
    pub battery_id: i64,
    pub kind: String,
    pub measured_at: String,
    pub capacity_mah: Option<i64>,
    pub ir_mohm: Option<i64>,
    pub instrument_id: Option<i64>,
    pub mode_id: Option<i64>,
    pub discharge_ma: Option<i64>,
    pub charge_ma: Option<i64>,
    pub notes: String,
    pub created_at: String,
}

fn measurement_from_row(row: &Row) -> DbResult<Measurement> {
    Ok(Measurement {
        id: row.get("id")?,
        battery_id: row.get("battery_id")?,
        kind: row.get("kind")?,
        measured_at: row.get("measured_at")?,
        capacity_mah: row.get("capacity_mah")?,
        ir_mohm: row.get("ir_mohm")?,
        instrument_id: row.get("instrument_id")?,
        mode_id: row.get("mode_id")?,
        discharge_ma: row.get("discharge_ma")?,
        charge_ma: row.get("charge_ma")?,
        notes: row.get("notes")?,
        created_at: row.get("created_at")?,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Instrument {
    pub id: i64,
    pub name: String,
    pub slots: i64,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default)]
    pub modes: Vec<Mode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Mode {
    pub id: i64,
    pub instrument_id: i64,
    pub name: String,
    pub kind: String,
    pub details: String,
    pub position: i64,
    pub created_at: String,
    pub updated_at: String,
    pub fields: String,
}

fn mode_from_row(row: &Row) -> DbResult<Mode> {
    Ok(Mode {
        id: row.get("id")?,
        instrument_id: row.get("instrument_id")?,
        name: row.get("name")?,
        kind: row.get("kind")?,
        details: row.get("details")?,
        position: row.get("position")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
        fields: row.get("fields")?,
    })
}

// ── Measurement kinds / default field config ────────────────────────────

pub const KINDS: [&str; 3] = ["bought", "charged", "analysed"];
pub const PROCEDURE_KINDS: [&str; 2] = ["charged", "analysed"];

/// Default field visibility/required config for a procedure kind, as the
/// same JSON shape the `fields` column stores.
pub fn default_fields(kind: &str) -> Json {
    fn field(visible: bool, required: bool) -> Json {
        serde_json::json!({ "visible": visible, "required": required })
    }
    match kind {
        "charged" => serde_json::json!({
            "charge_ma": field(true, false),
            "discharge_ma": field(true, false),
            "notes": field(true, false),
        }),
        "analysed" => serde_json::json!({
            "capacity_mah": field(true, false),
            "ir_mohm": field(true, false),
            "discharge_ma": field(true, false),
            "charge_ma": field(true, false),
            "notes": field(true, false),
        }),
        _ => serde_json::json!({}),
    }
}

/// Parse a mode's stored `fields` JSON, falling back to the kind's default
/// when it's empty/unparsable — mirrors `db.get_mode_fields`.
pub fn mode_fields(mode: &Mode) -> Json {
    if !mode.fields.is_empty() && mode.fields != "{}" {
        if let Ok(v) = serde_json::from_str::<Json>(&mode.fields) {
            return v;
        }
    }
    default_fields(&mode.kind)
}

fn clean_slots(slots: i64) -> i64 {
    if slots >= 1 {
        slots
    } else {
        1
    }
}

// ── Reads ────────────────────────────────────────────────────────────────

impl Db {
    pub fn next_id(&self) -> DbResult<i64> {
        let highest: Option<i64> = self
            .conn
            .query_row("SELECT MAX(id) FROM batteries", [], |r| r.get(0))?;
        Ok(highest.unwrap_or(0) + 1)
    }

    fn filter_clause(
        type_filter: &str,
        location_filter: &str,
        search: &str,
        show: &str,
    ) -> (String, Vec<String>) {
        let mut clause = String::from(" WHERE 1=1");
        let mut params: Vec<String> = Vec::new();

        match show {
            "active" => clause.push_str(" AND deleted_at IS NULL"),
            "deleted" => clause.push_str(" AND deleted_at IS NOT NULL"),
            _ => {}
        }
        if !type_filter.is_empty() {
            clause.push_str(" AND type = ?");
            params.push(type_filter.to_string());
        }
        if !location_filter.is_empty() {
            clause.push_str(" AND location = ?");
            params.push(location_filter.to_string());
        }
        if !search.is_empty() {
            clause.push_str(" AND (brand LIKE ? OR notes LIKE ? OR CAST(id AS TEXT) LIKE ?)");
            let pattern = format!("%{}%", search);
            params.push(pattern.clone());
            params.push(pattern.clone());
            params.push(pattern);
        }
        (clause, params)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn list_batteries(
        &self,
        type_filter: &str,
        location_filter: &str,
        search: &str,
        show: &str,
        sort: &str,
        limit: Option<i64>,
        offset: i64,
    ) -> DbResult<Vec<Battery>> {
        let (clause, mut params) = Self::filter_clause(type_filter, location_filter, search, show);
        let order_by = match sort {
            "type" => "type",
            "location" => "location",
            _ => "id",
        };
        let mut query = format!("SELECT * FROM batteries{} ORDER BY {}, id", clause, order_by);
        if let Some(limit) = limit {
            query.push_str(" LIMIT ? OFFSET ?");
            params.push(limit.to_string());
            params.push(offset.to_string());
        }
        let mut stmt = self.conn.prepare(&query)?;
        let rows = stmt.query_map(params_from_iter(params.iter()), battery_from_row)?;
        rows.collect()
    }

    pub fn count_batteries(
        &self,
        type_filter: &str,
        location_filter: &str,
        search: &str,
        show: &str,
    ) -> DbResult<i64> {
        let (clause, params) = Self::filter_clause(type_filter, location_filter, search, show);
        let query = format!("SELECT COUNT(*) FROM batteries{}", clause);
        self.conn
            .query_row(&query, params_from_iter(params.iter()), |r| r.get(0))
    }

    pub fn get(&self, battery_id: i64) -> DbResult<Option<Battery>> {
        self.conn
            .query_row("SELECT * FROM batteries WHERE id = ?", [battery_id], battery_from_row)
            .optional()
    }

    pub fn adjacent_ids(&self, battery_id: i64) -> DbResult<(Option<i64>, Option<i64>)> {
        let prev: Option<i64> = self.conn.query_row(
            "SELECT MAX(id) FROM batteries WHERE id < ?",
            [battery_id],
            |r| r.get(0),
        )?;
        let next: Option<i64> = self.conn.query_row(
            "SELECT MIN(id) FROM batteries WHERE id > ?",
            [battery_id],
            |r| r.get(0),
        )?;
        Ok((prev, next))
    }

    pub fn history(&self, battery_id: i64) -> DbResult<Vec<LocationEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT location, moved_at FROM location_history \
             WHERE battery_id = ? ORDER BY moved_at DESC, id DESC",
        )?;
        let rows = stmt.query_map([battery_id], |row| {
            Ok(LocationEntry {
                location: row.get("location")?,
                moved_at: row.get("moved_at")?,
            })
        })?;
        rows.collect()
    }

    fn check_column(column: &str) -> DbResult<()> {
        if !matches!(column, "type" | "brand" | "location") {
            return Err(rusqlite::Error::InvalidParameterName(column.to_string()));
        }
        Ok(())
    }

    pub fn distinct(&self, column: &str) -> DbResult<Vec<String>> {
        Self::check_column(column)?;
        let query = format!(
            "SELECT DISTINCT {c} AS value FROM batteries \
             WHERE {c} != '' AND deleted_at IS NULL ORDER BY {c}",
            c = column
        );
        let mut stmt = self.conn.prepare(&query)?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        rows.collect()
    }

    pub fn popular(&self, column: &str, limit: i64) -> DbResult<Vec<String>> {
        Self::check_column(column)?;
        let query = format!(
            "SELECT {c} AS value, COUNT(*) AS uses FROM batteries \
             WHERE {c} != '' AND deleted_at IS NULL \
             GROUP BY {c} ORDER BY uses DESC, {c} LIMIT ?",
            c = column
        );
        let mut stmt = self.conn.prepare(&query)?;
        let rows = stmt.query_map([limit], |r| r.get::<_, String>(0))?;
        rows.collect()
    }

    pub fn summary(&self) -> DbResult<Vec<(String, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT type, COUNT(*) AS count FROM batteries \
             WHERE deleted_at IS NULL GROUP BY type ORDER BY type",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
        rows.collect()
    }

    pub fn ids_in_use(&self, ids: &[i64]) -> DbResult<Vec<i64>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = vec!["?"; ids.len()].join(",");
        let query = format!("SELECT id FROM batteries WHERE id IN ({})", placeholders);
        let mut stmt = self.conn.prepare(&query)?;
        let rows = stmt.query_map(params_from_iter(ids.iter()), |r| r.get::<_, i64>(0))?;
        let mut out: Vec<i64> = rows.collect::<DbResult<_>>()?;
        out.sort_unstable();
        Ok(out)
    }

    pub fn deleted_count(&self) -> DbResult<i64> {
        self.conn.query_row(
            "SELECT COUNT(*) FROM batteries WHERE deleted_at IS NOT NULL",
            [],
            |r| r.get(0),
        )
    }

    // ── Writes ───────────────────────────────────────────────────────────

    pub fn create(
        &self,
        battery_id: i64,
        type_name: &str,
        nominal_mah: Option<i64>,
        brand: &str,
        location: &str,
        notes: &str,
    ) -> DbResult<Battery> {
        let timestamp = now();
        let type_name = type_name.trim().to_uppercase();
        let brand = brand.trim();
        let location = location.trim();
        let notes = notes.trim();
        self.conn.execute(
            "INSERT INTO batteries \
             (id, type, nominal_mah, brand, location, notes, created_at, updated_at, deleted_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, NULL)",
            params![battery_id, type_name, nominal_mah, brand, location, notes, timestamp, timestamp],
        )?;
        if !location.is_empty() {
            self.conn.execute(
                "INSERT INTO location_history (battery_id, location, moved_at) VALUES (?, ?, ?)",
                params![battery_id, location, timestamp],
            )?;
        }
        Ok(self.get(battery_id)?.expect("just inserted"))
    }

    pub fn set_location(&self, battery_id: i64, location: &str) -> DbResult<Option<Battery>> {
        let battery = match self.get(battery_id)? {
            Some(b) => b,
            None => return Ok(None),
        };
        let location = location.trim();
        if location == battery.location {
            return Ok(Some(battery));
        }
        let timestamp = now();
        self.conn.execute(
            "UPDATE batteries SET location = ?, updated_at = ? WHERE id = ?",
            params![location, timestamp, battery_id],
        )?;
        self.conn.execute(
            "INSERT INTO location_history (battery_id, location, moved_at) VALUES (?, ?, ?)",
            params![battery_id, location, timestamp],
        )?;
        self.get(battery_id)
    }

    /// Edit descriptive fields. `nominal_mah` is `Some(None)` to clear it,
    /// `None` to leave it untouched — each field is independently
    /// updated-or-left-alone, not all-or-nothing.
    pub fn update(
        &self,
        battery_id: i64,
        type_name: Option<&str>,
        nominal_mah: Option<Option<i64>>,
        brand: Option<&str>,
        notes: Option<&str>,
    ) -> DbResult<Option<Battery>> {
        if type_name.is_none() && nominal_mah.is_none() && brand.is_none() && notes.is_none() {
            return self.get(battery_id);
        }
        let mut assignments = Vec::new();
        let mut values: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if let Some(t) = type_name {
            assignments.push("type = ?");
            values.push(Box::new(t.trim().to_uppercase()));
        }
        if let Some(n) = nominal_mah {
            assignments.push("nominal_mah = ?");
            values.push(Box::new(n));
        }
        if let Some(b) = brand {
            assignments.push("brand = ?");
            values.push(Box::new(b.to_string()));
        }
        if let Some(nt) = notes {
            assignments.push("notes = ?");
            values.push(Box::new(nt.to_string()));
        }
        assignments.push("updated_at = ?");
        values.push(Box::new(now()));
        values.push(Box::new(battery_id));

        let query = format!("UPDATE batteries SET {} WHERE id = ?", assignments.join(", "));
        let params: Vec<&dyn rusqlite::ToSql> = values.iter().map(|b| b.as_ref()).collect();
        self.conn.execute(&query, params.as_slice())?;
        self.get(battery_id)
    }

    /// Mirrors `db.soft_delete`: hard-deletes only when `battery_id` is the
    /// current max id and has no measurements; otherwise soft-deletes.
    pub fn soft_delete(&self, battery_id: i64) -> DbResult<Option<Battery>> {
        let battery = match self.get(battery_id)? {
            Some(b) => b,
            None => return Ok(None),
        };
        if battery.deleted_at.is_some() {
            return Ok(Some(Battery {
                hard_deleted: Some(false),
                ..battery
            }));
        }

        let highest: Option<i64> = self
            .conn
            .query_row("SELECT MAX(id) FROM batteries", [], |r| r.get(0))?;
        let has_measurements: bool = self
            .conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM measurements WHERE battery_id = ?)",
                [battery_id],
                |r| r.get(0),
            )?;

        if Some(battery_id) == highest && !has_measurements {
            self.conn
                .execute("DELETE FROM batteries WHERE id = ?", [battery_id])?;
            return Ok(Some(Battery {
                id: battery_id,
                r#type: String::new(),
                nominal_mah: None,
                brand: String::new(),
                location: String::new(),
                notes: String::new(),
                created_at: String::new(),
                updated_at: String::new(),
                deleted_at: None,
                hard_deleted: Some(true),
            }));
        }

        let timestamp = now();
        self.conn.execute(
            "UPDATE batteries SET deleted_at = ?, updated_at = ? WHERE id = ?",
            params![timestamp, timestamp, battery_id],
        )?;
        let mut result = self.get(battery_id)?.expect("row still exists");
        result.hard_deleted = Some(false);
        Ok(Some(result))
    }

    /// Permanently removes a deleted battery that was never measured. Unlike
    /// `soft_delete`'s automatic hard-delete (which only fires for the
    /// highest id), this is reachable for any id but only once it is already
    /// soft-deleted and still has no readings — both checked here too, not
    /// just by the caller, since this is the one action that cannot be
    /// undone.
    pub fn purge(&self, battery_id: i64) -> Result<(), String> {
        let battery = self.get(battery_id).map_err(|e| e.to_string())?;
        let Some(battery) = battery else { return Err("No such battery.".to_string()) };
        if battery.deleted_at.is_none() {
            return Err("Only a deleted battery can be permanently removed.".to_string());
        }
        let has_measurements: bool = self
            .conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM measurements WHERE battery_id = ?)",
                [battery_id],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if has_measurements {
            return Err("This battery has readings, so it cannot be permanently removed.".to_string());
        }
        self.conn
            .execute("DELETE FROM location_history WHERE battery_id = ?", [battery_id])
            .map_err(|e| e.to_string())?;
        self.conn
            .execute("DELETE FROM batteries WHERE id = ?", [battery_id])
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn restore(&self, battery_id: i64) -> DbResult<Option<Battery>> {
        let battery = match self.get(battery_id)? {
            Some(b) => b,
            None => return Ok(None),
        };
        if battery.deleted_at.is_none() {
            return Ok(Some(battery));
        }
        self.conn.execute(
            "UPDATE batteries SET deleted_at = NULL, updated_at = ? WHERE id = ?",
            params![now(), battery_id],
        )?;
        self.get(battery_id)
    }

    pub fn reset(&self, battery_id: i64) -> DbResult<Option<Battery>> {
        if self.get(battery_id)?.is_none() {
            return Ok(None);
        }
        let timestamp = now();
        self.conn
            .execute("DELETE FROM location_history WHERE battery_id = ?", [battery_id])?;
        self.conn
            .execute("DELETE FROM measurements WHERE battery_id = ?", [battery_id])?;
        self.conn.execute(
            "UPDATE batteries SET nominal_mah = NULL, brand = '', location = '', notes = '', \
             created_at = ?, updated_at = ?, deleted_at = NULL WHERE id = ?",
            params![timestamp, timestamp, battery_id],
        )?;
        self.conn.execute(
            "INSERT INTO location_history (battery_id, location, moved_at) VALUES (?, ?, ?)",
            params![battery_id, "Record cleared", timestamp],
        )?;
        self.get(battery_id)
    }

    // ── Measurements ─────────────────────────────────────────────────────

    #[allow(clippy::too_many_arguments)]
    pub fn add_measurement(
        &self,
        battery_id: i64,
        kind: &str,
        measured_at: &str,
        capacity_mah: Option<i64>,
        ir_mohm: Option<i64>,
        instrument_id: Option<i64>,
        mode_id: Option<i64>,
        discharge_ma: Option<i64>,
        charge_ma: Option<i64>,
        notes: &str,
    ) -> DbResult<Option<Measurement>> {
        if !KINDS.contains(&kind) {
            return Err(rusqlite::Error::InvalidParameterName(format!(
                "{kind:?} is not a kind of measurement."
            )));
        }
        if self.get(battery_id)?.is_none() {
            return Ok(None);
        }
        self.conn.execute(
            "INSERT INTO measurements \
             (battery_id, kind, measured_at, capacity_mah, ir_mohm, instrument_id, mode_id, \
              discharge_ma, charge_ma, notes, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                battery_id,
                kind,
                measured_at,
                capacity_mah,
                ir_mohm,
                instrument_id,
                mode_id,
                discharge_ma,
                charge_ma,
                notes.trim(),
                now()
            ],
        )?;
        let new_id = self.conn.last_insert_rowid();
        self.measurement(new_id)
    }

    pub fn last_reading_defaults(
        &self,
    ) -> DbResult<(Option<i64>, Option<i64>, Option<i64>, Option<i64>)> {
        self.conn
            .query_row(
                "SELECT instrument_id, mode_id, discharge_ma, charge_ma FROM measurements \
                 WHERE instrument_id IS NOT NULL ORDER BY id DESC LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()
            .map(|opt| opt.unwrap_or((None, None, None, None)))
    }

    pub fn measurement(&self, id: i64) -> DbResult<Option<Measurement>> {
        self.conn
            .query_row(
                "SELECT * FROM measurements WHERE id = ?",
                [id],
                measurement_from_row,
            )
            .optional()
    }

    pub fn measurements(&self, battery_id: i64) -> DbResult<Vec<Measurement>> {
        let mut stmt = self.conn.prepare(
            "SELECT * FROM measurements WHERE battery_id = ? ORDER BY measured_at DESC, id DESC",
        )?;
        let rows = stmt.query_map([battery_id], measurement_from_row)?;
        rows.collect()
    }

    pub fn delete_measurement(&self, id: i64) -> DbResult<Option<i64>> {
        let row = self.measurement(id)?;
        let battery_id = match row {
            Some(m) => m.battery_id,
            None => return Ok(None),
        };
        self.conn.execute("DELETE FROM measurements WHERE id = ?", [id])?;
        Ok(Some(battery_id))
    }

    // ── Instruments / procedures ────────────────────────────────────────

    pub fn add_instrument(&self, name: &str, slots: i64) -> DbResult<Instrument> {
        let name = name.trim();
        if name.is_empty() {
            return Err(rusqlite::Error::InvalidParameterName(
                "The name is required.".into(),
            ));
        }
        let timestamp = now();
        self.conn.execute(
            "INSERT INTO instruments (name, slots, created_at, updated_at) VALUES (?, ?, ?, ?)",
            params![name, clean_slots(slots), timestamp, timestamp],
        )?;
        let new_id = self.conn.last_insert_rowid();
        Ok(self.instrument(new_id)?.expect("just inserted"))
    }

    pub fn instrument(&self, instrument_id: i64) -> DbResult<Option<Instrument>> {
        let base = self
            .conn
            .query_row(
                "SELECT * FROM instruments WHERE id = ?",
                [instrument_id],
                |row| {
                    Ok(Instrument {
                        id: row.get("id")?,
                        name: row.get("name")?,
                        slots: row.get("slots")?,
                        created_at: row.get("created_at")?,
                        updated_at: row.get("updated_at")?,
                        modes: Vec::new(),
                    })
                },
            )
            .optional()?;
        let mut instrument = match base {
            Some(i) => i,
            None => return Ok(None),
        };
        let mut stmt = self.conn.prepare(
            "SELECT * FROM instrument_modes WHERE instrument_id = ? ORDER BY position, id",
        )?;
        let rows = stmt.query_map([instrument_id], mode_from_row)?;
        instrument.modes = rows.collect::<DbResult<_>>()?;
        Ok(Some(instrument))
    }

    pub fn list_instruments(&self) -> DbResult<Vec<Instrument>> {
        let ids: Vec<i64> = {
            let mut stmt = self.conn.prepare("SELECT id FROM instruments ORDER BY name")?;
            let rows = stmt.query_map([], |r| r.get::<_, i64>(0))?;
            rows.collect::<DbResult<_>>()?
        };
        ids.into_iter()
            .map(|id| Ok(self.instrument(id)?.expect("id just listed")))
            .collect()
    }

    pub fn update_instrument(
        &self,
        instrument_id: i64,
        name: &str,
        slots: i64,
    ) -> DbResult<Option<Instrument>> {
        if self.instrument(instrument_id)?.is_none() {
            return Ok(None);
        }
        let name = name.trim();
        if name.is_empty() {
            return Err(rusqlite::Error::InvalidParameterName(
                "The name is required.".into(),
            ));
        }
        self.conn.execute(
            "UPDATE instruments SET name = ?, slots = ?, updated_at = ? WHERE id = ?",
            params![name, clean_slots(slots), now(), instrument_id],
        )?;
        self.instrument(instrument_id)
    }

    pub fn delete_instrument(&self, instrument_id: i64) -> DbResult<bool> {
        if self.instrument(instrument_id)?.is_none() {
            return Ok(false);
        }
        self.conn
            .execute("DELETE FROM instruments WHERE id = ?", [instrument_id])?;
        Ok(true)
    }

    pub fn add_mode(
        &self,
        instrument_id: i64,
        name: &str,
        kind: &str,
        details: &str,
        fields: Option<&Json>,
    ) -> DbResult<Mode> {
        if self.instrument(instrument_id)?.is_none() {
            return Err(rusqlite::Error::InvalidParameterName(format!(
                "No instrument has id {instrument_id}."
            )));
        }
        let name = name.trim();
        if name.is_empty() {
            return Err(rusqlite::Error::InvalidParameterName(
                "The name is required.".into(),
            ));
        }
        if !PROCEDURE_KINDS.contains(&kind) {
            return Err(rusqlite::Error::InvalidParameterName(format!(
                "Kind must be one of {}.",
                PROCEDURE_KINDS.join(", ")
            )));
        }
        let fields_json = match fields {
            Some(f) => f.to_string(),
            None => default_fields(kind).to_string(),
        };
        let timestamp = now();
        let next_position: i64 = self.conn.query_row(
            "SELECT COALESCE(MAX(position) + 1, 0) FROM instrument_modes WHERE instrument_id = ?",
            [instrument_id],
            |r| r.get(0),
        )?;
        self.conn.execute(
            "INSERT INTO instrument_modes \
             (instrument_id, name, kind, details, fields, position, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                instrument_id,
                name,
                kind,
                details.trim(),
                fields_json,
                next_position,
                timestamp,
                timestamp
            ],
        )?;
        let new_id = self.conn.last_insert_rowid();
        Ok(self.mode(new_id)?.expect("just inserted"))
    }

    pub fn mode(&self, mode_id: i64) -> DbResult<Option<Mode>> {
        self.conn
            .query_row(
                "SELECT * FROM instrument_modes WHERE id = ?",
                [mode_id],
                mode_from_row,
            )
            .optional()
    }

    pub fn update_mode(
        &self,
        mode_id: i64,
        name: &str,
        kind: &str,
        details: &str,
        fields: Option<&Json>,
    ) -> DbResult<Option<Mode>> {
        let existing = match self.mode(mode_id)? {
            Some(m) => m,
            None => return Ok(None),
        };
        let name = name.trim();
        if name.is_empty() {
            return Err(rusqlite::Error::InvalidParameterName(
                "The name is required.".into(),
            ));
        }
        if !PROCEDURE_KINDS.contains(&kind) {
            return Err(rusqlite::Error::InvalidParameterName(format!(
                "Kind must be one of {}.",
                PROCEDURE_KINDS.join(", ")
            )));
        }
        let fields_json = match fields {
            Some(f) => f.to_string(),
            None => mode_fields(&existing).to_string(),
        };
        self.conn.execute(
            "UPDATE instrument_modes SET name = ?, kind = ?, details = ?, fields = ?, \
             updated_at = ? WHERE id = ?",
            params![name, kind, details.trim(), fields_json, now(), mode_id],
        )?;
        self.mode(mode_id)
    }

    pub fn delete_mode(&self, mode_id: i64) -> DbResult<bool> {
        if self.mode(mode_id)?.is_none() {
            return Ok(false);
        }
        self.conn
            .execute("DELETE FROM instrument_modes WHERE id = ?", [mode_id])?;
        Ok(true)
    }

    /// Swaps position with the adjacent sibling in current display order, so
    /// this keeps working even after a deletion has left position values
    /// with a gap in them. Returns `false` (not an error) when unknown or
    /// already at that end.
    pub fn move_mode(&self, mode_id: i64, direction: &str) -> DbResult<bool> {
        if direction != "up" && direction != "down" {
            return Err(rusqlite::Error::InvalidParameterName(format!(
                "{direction:?} is not a direction."
            )));
        }
        let current = match self.mode(mode_id)? {
            Some(m) => m,
            None => return Ok(false),
        };
        let siblings: Vec<(i64, i64)> = {
            let mut stmt = self.conn.prepare(
                "SELECT id, position FROM instrument_modes WHERE instrument_id = ? \
                 ORDER BY position, id",
            )?;
            let rows = stmt.query_map([current.instrument_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect::<DbResult<_>>()?
        };
        let index = siblings.iter().position(|(id, _)| *id == mode_id).unwrap();
        let target = if direction == "up" {
            index.checked_sub(1)
        } else {
            Some(index + 1)
        };
        let target = match target {
            Some(t) if t < siblings.len() => t,
            _ => return Ok(false),
        };
        let timestamp = now();
        let (here_id, here_pos) = siblings[index];
        let (there_id, there_pos) = siblings[target];
        self.conn.execute(
            "UPDATE instrument_modes SET position = ?, updated_at = ? WHERE id = ?",
            params![there_pos, timestamp, here_id],
        )?;
        self.conn.execute(
            "UPDATE instrument_modes SET position = ?, updated_at = ? WHERE id = ?",
            params![here_pos, timestamp, there_id],
        )?;
        Ok(true)
    }

    // ── Settings ─────────────────────────────────────────────────────────

    pub fn get_setting(&self, key: &str) -> DbResult<Option<String>> {
        self.conn
            .query_row("SELECT value FROM settings WHERE key = ?", [key], |r| r.get(0))
            .optional()
    }

    pub fn set_setting(&self, key: &str, value: &str) -> DbResult<()> {
        self.conn.execute(
            "INSERT INTO settings (key, value, updated_at) VALUES (?, ?, ?) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            params![key, value, now()],
        )?;
        Ok(())
    }

    pub fn delete_setting(&self, key: &str) -> DbResult<()> {
        self.conn.execute("DELETE FROM settings WHERE key = ?", [key])?;
        Ok(())
    }

    /// Write a consistent copy of the database to `destination` using
    /// SQLite's own backup API (safe on a live database), returning the
    /// path actually written. A directory destination gets a timestamped
    /// file inside it.
    pub fn backup(&self, destination: &Path) -> rusqlite::Result<std::path::PathBuf> {
        let mut destination = destination.to_path_buf();
        if destination.is_dir() {
            let stamp = chrono::Local::now().format("%Y-%m-%d-%H%M%S");
            destination = destination.join(format!("batteries-{stamp}.db"));
        }
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let mut target = Connection::open(&destination)?;
        let backup = rusqlite::backup::Backup::new(&self.conn, &mut target)?;
        backup.run_to_completion(5, std::time::Duration::from_millis(250), None)?;
        Ok(destination)
    }
}
