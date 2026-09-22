//! One place that decides what every adjustable setting currently is.
//!
//! Three layers, checked in order: the `settings` DB table, then the
//! matching environment variable, then the built-in default. Saving a blank
//! non-bool value deletes the override rather than storing one, so the
//! setting starts tracking the environment/default again.

use std::fmt;

use serde::Serialize;

use crate::db::Db;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    Bool,
    Int { min: i64, max: i64 },
    Float { min: f64, max: f64 },
    Choice { choices: &'static [&'static str] },
    Text,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum Value {
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(String),
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Bool(b) => write!(f, "{}", if *b { "1" } else { "0" }),
            Value::Int(i) => write!(f, "{i}"),
            Value::Float(v) => write!(f, "{v}"),
            Value::Text(s) => write!(f, "{s}"),
        }
    }
}

impl Value {
    pub fn as_bool(&self) -> bool {
        match self {
            Value::Bool(b) => *b,
            _ => false,
        }
    }
    pub fn as_i64(&self) -> i64 {
        match self {
            Value::Int(i) => *i,
            _ => 0,
        }
    }
    pub fn as_f64(&self) -> f64 {
        match self {
            Value::Float(v) => *v,
            Value::Int(i) => *i as f64,
            _ => 0.0,
        }
    }
    pub fn as_text(&self) -> String {
        match self {
            Value::Text(s) => s.clone(),
            other => other.to_string(),
        }
    }
}

pub struct SettingSpec {
    pub key: &'static str,
    pub env: &'static str,
    /// String form of the default, coerced the same way a stored/env value
    /// is (so `"1"`/`"0"` for bool, matching `TRUE_WORDS`).
    pub default: &'static str,
    pub kind: Kind,
    pub group: &'static str,
    pub label: &'static str,
    pub help: &'static str,
}

const METHOD_NOTE: &str = "These thresholds only mean something against the way you actually \
measure. A 1 kHz alternating-current tester reports roughly half what the direct-current method \
of loading the cell and dividing the voltage drop by the current gives, so a limit set for one \
will be far too loose or far too tight for the other. Set it against your own readings of a cell \
you know is good rather than against a number from a datasheet.";

pub const SETTINGS: &[SettingSpec] = &[
    SettingSpec {
        key: "printer_host",
        env: "PTOUCH_HOST",
        default: "",
        kind: Kind::Text,
        group: "printer",
        label: "Printer address",
        help: "Hostname or IP of a network Brother PT-E550W, PT-P750W or PT-P710BT — the only \
               printers this app's raster protocol is written against. It is contacted on port \
               9100. Other Brother label printers may or may not work correctly; anything else \
               (other brands, or no printer at all) should use \"PDF\" under Label output \
               instead. Leave empty if using USB, or PDF output.",
    },
    SettingSpec {
        key: "printer_usb",
        env: "PTOUCH_USB",
        default: "0",
        kind: Kind::Bool,
        group: "printer",
        label: "Use a USB printer",
        help: "Only consulted when no printer address is set. Requires the printer to be \
               attached to the machine running this application.",
    },
    SettingSpec {
        key: "tape_mm",
        env: "TAPE_MM",
        default: "9",
        kind: Kind::Choice {
            choices: &["3.5", "6", "9", "12", "18", "24"],
        },
        group: "labels",
        label: "Tape width (mm)",
        help: "Must match the cartridge actually loaded. It decides how tall the label image is \
               rendered, so getting it wrong produces labels the wrong size.",
    },
    SettingSpec {
        key: "id_text_scale",
        env: "ID_TEXT_SCALE",
        default: "0.40",
        kind: Kind::Float { min: 0.15, max: 0.90 },
        group: "labels",
        label: "Printed id size",
        help: "How much of the tape height the number beside the code takes up, as a fraction. \
               0.40 is about 2.8 mm on 9 mm tape.",
    },
    SettingSpec {
        key: "chain_labels",
        env: "CHAIN_LABELS",
        default: "1",
        kind: Kind::Bool,
        group: "labels",
        label: "Chain print batches",
        help: "When several labels are printed at once, feed the tape out once at the end \
               rather than after each label.",
    },
    SettingSpec {
        key: "half_cut",
        env: "HALF_CUT",
        default: "1",
        kind: Kind::Bool,
        group: "labels",
        label: "Half cut between chained labels",
        help: "Score the backing between labels so the strip tears apart cleanly. Turn this off \
               to have the printer cut all the way through; that still chains.",
    },
    SettingSpec {
        key: "health_limit_pct",
        env: "HEALTH_LIMIT_PCT",
        default: "80",
        kind: Kind::Int { min: 1, max: 100 },
        group: "condition",
        label: "Health warning below (%)",
        help: "A cell whose measured capacity has fallen below this share of its nominal \
               capacity is flagged on its page.",
    },
    SettingSpec {
        key: "ir_limit_aa",
        env: "IR_LIMIT_AA",
        default: "100",
        kind: Kind::Int { min: 1, max: 5000 },
        group: "condition",
        label: "Resistance limit for AA cells (m\u{3a9})",
        help: METHOD_NOTE,
    },
    SettingSpec {
        key: "ir_limit_aaa",
        env: "IR_LIMIT_AAA",
        default: "200",
        kind: Kind::Int { min: 1, max: 5000 },
        group: "condition",
        label: "Resistance limit for AAA cells (m\u{3a9})",
        help: METHOD_NOTE,
    },
    SettingSpec {
        key: "list_view",
        env: "LIST_VIEW",
        default: "table",
        kind: Kind::Choice {
            choices: &["table", "grid"],
        },
        group: "list",
        label: "List layout",
        help: "The table gives one battery per line; the grid gives each battery a tile.",
    },
    SettingSpec {
        key: "grid_columns",
        env: "GRID_COLUMNS",
        default: "3",
        kind: Kind::Int { min: 1, max: 8 },
        group: "list",
        label: "Batteries per row in the grid layout",
        help: "A maximum rather than a fixed number. Ignored while the table layout is in use.",
    },
    SettingSpec {
        key: "page_size",
        env: "PAGE_SIZE",
        default: "25",
        kind: Kind::Int { min: 5, max: 500 },
        group: "list",
        label: "Batteries per page",
        help: "How many batteries the list shows before paging, in either layout.",
    },
    SettingSpec {
        key: "print_on_add",
        env: "PRINT_ON_ADD",
        default: "1",
        kind: Kind::Bool,
        group: "labels",
        label: "Print labels when adding batteries",
        help: "Turn this off while the printer is out of tape or packed away.",
    },
    SettingSpec {
        key: "label_output",
        env: "LABEL_OUTPUT",
        default: "printer",
        kind: Kind::Choice {
            choices: &["printer", "pdf"],
        },
        group: "labels",
        label: "Label output",
        help: "\"Printer\" sends labels straight to the network printer above. \"PDF\" saves \
               them to a file instead, via a native Save dialog, to print through whatever \
               software came with your own printer — the only option if it isn't a Brother \
               PT-E550W/P750W/P710BT. Applies everywhere a label is printed: adding a battery, \
               reprinting one, and the batch Print page.",
    },
    SettingSpec {
        key: "pdf_layout",
        env: "PDF_LAYOUT",
        default: "pages",
        kind: Kind::Choice {
            choices: &["pages", "grid"],
        },
        group: "labels",
        label: "PDF layout",
        help: "Only matters when Label output is PDF and more than one label is being saved at \
               once. \"One page per label\" sizes each page exactly to its label, so printing \
               at 100% scale gives the correct physical size — the same guarantee a single \
               label always has. \"Grid\" tiles several labels onto A4 pages instead, for \
               printing a whole sheet at once.",
    },
];

pub fn spec(key: &str) -> Option<&'static SettingSpec> {
    SETTINGS.iter().find(|s| s.key == key)
}

const TRUE_WORDS: [&str; 4] = ["1", "true", "yes", "on"];
const FALSE_WORDS: [&str; 4] = ["0", "false", "no", "off"];

fn as_bool(raw: &str, label: &str) -> Result<bool, String> {
    let text = raw.trim().to_lowercase();
    if TRUE_WORDS.contains(&text.as_str()) {
        Ok(true)
    } else if FALSE_WORDS.contains(&text.as_str()) {
        Ok(false)
    } else {
        Err(format!("{label} must be on or off, and {raw:?} is neither."))
    }
}

/// Turn a stored or environment string into the setting's typed value.
pub fn coerce(spec: &SettingSpec, raw: &str) -> Result<Value, String> {
    match spec.kind {
        Kind::Bool => as_bool(raw, spec.label).map(Value::Bool),
        Kind::Int { min, max } => {
            let text = raw.trim();
            let digits = text.trim_start_matches(['+', '-']);
            if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
                return Err(format!("{} must be a whole number, and {raw:?} is not.", spec.label));
            }
            let value: i64 = text
                .parse()
                .map_err(|_| format!("{} must be a whole number, and {raw:?} is not.", spec.label))?;
            if value < min || value > max {
                return Err(format!(
                    "{} must be between {min} and {max}, and {value} is outside that.",
                    spec.label
                ));
            }
            Ok(Value::Int(value))
        }
        Kind::Float { min, max } => {
            let text = raw.replace(',', ".");
            let value: f64 = text
                .trim()
                .parse()
                .map_err(|_| format!("{} must be a number, and {raw:?} is not.", spec.label))?;
            if value < min || value > max {
                return Err(format!(
                    "{} must be between {min} and {max}, and {value} is outside that.",
                    spec.label
                ));
            }
            Ok(Value::Float(value))
        }
        Kind::Choice { choices } => {
            let text = raw.trim();
            if !choices.contains(&text) {
                return Err(format!(
                    "{} must be one of {}, and {raw:?} is not.",
                    spec.label,
                    choices.join(", ")
                ));
            }
            Ok(Value::Text(text.to_string()))
        }
        Kind::Text => Ok(Value::Text(raw.trim().to_string())),
    }
}

pub fn source(db: &Db, key: &str) -> &'static str {
    let Some(spec) = spec(key) else { return "built-in" };
    if let Ok(Some(_)) = db.get_setting(key) {
        return "database";
    }
    if !std::env::var(spec.env).unwrap_or_default().trim().is_empty() {
        return "environment";
    }
    "built-in"
}

/// The value in force right now. A bad stored or environment value falls
/// back to the next layer rather than raising — the settings page validates
/// on the way in, so this only matters for a value edited by hand elsewhere.
pub fn get(db: &Db, key: &str) -> Value {
    let spec = match self::spec(key) {
        Some(s) => s,
        None => return Value::Text(String::new()),
    };

    if let Ok(Some(stored)) = db.get_setting(key) {
        if let Ok(value) = coerce(spec, &stored) {
            return value;
        }
    }

    let from_env = std::env::var(spec.env).unwrap_or_default();
    if !from_env.trim().is_empty() {
        if let Ok(value) = coerce(spec, &from_env) {
            return value;
        }
    }

    coerce(spec, spec.default).expect("built-in defaults are always valid")
}

pub fn fallback_text(key: &str) -> String {
    let Some(spec) = spec(key) else { return String::new() };
    let from_env = std::env::var(spec.env).unwrap_or_default();
    let (raw, from) = if !from_env.trim().is_empty() {
        (from_env, "environment")
    } else {
        (spec.default.to_string(), "built-in default")
    };
    let shown = if spec.kind == Kind::Bool {
        match as_bool(&raw, spec.label) {
            Ok(true) => "on".to_string(),
            Ok(false) => "off".to_string(),
            Err(_) => raw.clone(),
        }
    } else if raw.is_empty() {
        "empty".to_string()
    } else {
        raw.clone()
    };
    format!("{shown}, from the {from}")
}

/// Store an override, or remove it when the value is blank (non-bool only).
pub fn save(db: &Db, key: &str, raw: &str) -> Result<(), String> {
    let spec = self::spec(key).ok_or_else(|| format!("{key:?} is not a setting."))?;
    if spec.kind != Kind::Bool && raw.trim().is_empty() {
        db.delete_setting(key).map_err(|e| e.to_string())?;
        return Ok(());
    }
    let value = coerce(spec, raw)?;
    db.set_setting(key, &value.to_string()).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn clear(db: &Db, key: &str) -> Result<(), String> {
    db.delete_setting(key).map_err(|e| e.to_string())
}

#[derive(Debug, Clone, Serialize)]
pub struct SettingRow {
    pub key: String,
    pub label: String,
    pub help: String,
    pub kind: String,
    pub choices: Vec<String>,
    pub value: Value,
    pub text: String,
    pub source: String,
    pub fallback: String,
}

fn row(db: &Db, spec: &SettingSpec) -> SettingRow {
    let value = get(db, spec.key);
    let kind_name = match spec.kind {
        Kind::Bool => "bool",
        Kind::Int { .. } => "int",
        Kind::Float { .. } => "float",
        Kind::Choice { .. } => "choice",
        Kind::Text => "text",
    };
    let choices = match spec.kind {
        Kind::Choice { choices } => choices.iter().map(|s| s.to_string()).collect(),
        _ => Vec::new(),
    };
    let text = match spec.kind {
        Kind::Bool => if value.as_bool() { "1" } else { "0" }.to_string(),
        _ => value.to_string(),
    };
    SettingRow {
        key: spec.key.to_string(),
        label: spec.label.to_string(),
        help: spec.help.to_string(),
        kind: kind_name.to_string(),
        choices,
        value,
        text,
        source: source(db, spec.key).to_string(),
        fallback: fallback_text(spec.key),
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Section {
    pub key: String,
    pub title: String,
    pub blurb: String,
    pub settings: Vec<SettingRow>,
}

const GROUPS: &[(&str, &str, &str)] = &[
    (
        "printer",
        "Printer",
        "Where labels are sent. Nothing here is consulted until something is actually printed, \
         so a wrong address stops labels rather than the application.",
    ),
    (
        "labels",
        "Labels",
        "What comes out of the printer: how big a label is, and how several of them at once are \
         fed and cut.",
    ),
    (
        "condition",
        "Condition warnings",
        "The internal resistance above which a cell is flagged as suspect.",
    ),
    (
        "list",
        "The battery list",
        "How the list draws itself. None of this touches the batteries themselves.",
    ),
];

/// Everything the settings page needs, in sections: `GROUPS` order first,
/// each holding its settings in declaration order; anything naming an
/// unlisted group lands in a trailing section instead of vanishing.
pub fn overview(db: &Db) -> Vec<Section> {
    let mut sections: Vec<Section> = GROUPS
        .iter()
        .map(|(key, title, blurb)| Section {
            key: key.to_string(),
            title: title.to_string(),
            blurb: blurb.to_string(),
            settings: Vec::new(),
        })
        .collect();

    let mut orphans: Vec<Section> = Vec::new();
    for spec in SETTINGS {
        if let Some(section) = sections.iter_mut().find(|s| s.key == spec.group) {
            section.settings.push(row(db, spec));
        } else {
            let title = if spec.group.is_empty() {
                "Ungrouped".to_string()
            } else {
                let mut c = spec.group.replace('_', " ");
                if let Some(first) = c.get_mut(0..1) {
                    first.make_ascii_uppercase();
                }
                c
            };
            match orphans.iter_mut().find(|s| s.key == spec.group) {
                Some(section) => section.settings.push(row(db, spec)),
                None => orphans.push(Section {
                    key: spec.group.to_string(),
                    title,
                    blurb: "These settings name a section that does not exist, so they are \
                            shown here rather than not at all."
                        .to_string(),
                    settings: vec![row(db, spec)],
                }),
            }
        }
    }

    sections.retain(|s| !s.settings.is_empty());
    sections.extend(orphans);
    sections
}
