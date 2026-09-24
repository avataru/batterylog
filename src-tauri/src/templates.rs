//! Tera setup: templates embedded via `include_str!` (no filesystem lookup
//! at runtime, so install location never matters) plus custom
//! filters/globals (`static_url`, `icon`, the `markdown` filter) and a few
//! small helpers Tera's expression language doesn't have built in
//! (`%03d`-style zero padding, `.title()`, dict-get-with-default).

use std::collections::HashMap;

use tera::{Tera, Value};

use crate::icons;

fn describe_error(e: &tera::Error) -> String {
    use std::error::Error;
    let mut out = e.to_string();
    let mut source = e.source();
    while let Some(s) = source {
        out.push_str(" -> ");
        out.push_str(&s.to_string());
        source = s.source();
    }
    out
}

macro_rules! add {
    ($tera:expr, $name:expr, $path:expr) => {
        $tera
            .add_raw_template($name, include_str!($path))
            .unwrap_or_else(|e| panic!("template {} failed to parse: {}", $name, describe_error(&e)));
    };
}

pub fn build() -> Tera {
    let mut tera = Tera::default();

    add!(tera, "base.html", "../templates/base.html");
    add!(tera, "macros.html", "../templates/macros.html");
    add!(tera, "list.html", "../templates/list.html");
    add!(tera, "detail.html", "../templates/detail.html");
    add!(tera, "missing.html", "../templates/missing.html");
    add!(tera, "add.html", "../templates/add.html");
    add!(tera, "scan.html", "../templates/scan.html");
    add!(tera, "print.html", "../templates/print.html");
    add!(tera, "instruments.html", "../templates/instruments.html");
    add!(tera, "settings.html", "../templates/settings.html");
    add!(tera, "batch_measure.html", "../templates/batch_measure.html");
    add!(tera, "batch_location.html", "../templates/batch_location.html");
    add!(tera, "match.html", "../templates/match.html");
    add!(tera, "match_log.html", "../templates/match_log.html");
    add!(tera, "dialog_measure.html", "../templates/dialog_measure.html");
    add!(tera, "dialog_location.html", "../templates/dialog_location.html");
    add!(tera, "dialog_label.html", "../templates/dialog_label.html");

    tera.register_function("static_url", static_url_fn);
    tera.register_function("icon", icon_fn);
    tera.register_filter("markdown", markdown_filter);
    tera.register_filter("pad3", pad3_filter);
    tera.register_filter("title_case", title_case_filter);
    tera.register_filter("date10", date10_filter);
    tera.register_filter("dict_get", dict_get_filter);

    tera
}

fn static_url_fn(args: &HashMap<String, Value>) -> tera::Result<Value> {
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| tera::Error::msg("static_url: missing `name`"))?;
    // No cache-busting stamp needed: assets ship bundled with the app binary
    // and only change when the app itself is reinstalled/updated.
    Ok(Value::String(format!("/{name}")))
}

fn icon_fn(args: &HashMap<String, Value>) -> tera::Result<Value> {
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| tera::Error::msg("icon: missing `name`"))?;
    let cls = args.get("cls").and_then(|v| v.as_str()).unwrap_or("");
    let svg = icons::svg(name);
    let out = if !cls.is_empty() {
        svg.replacen("<svg ", &format!("<svg class=\"{cls}\" "), 1)
    } else {
        svg.to_string()
    };
    Ok(Value::String(out))
}

fn markdown_filter(value: &Value, _args: &HashMap<String, Value>) -> tera::Result<Value> {
    let text = value.as_str().unwrap_or("");
    if text.is_empty() {
        return Ok(Value::String(String::new()));
    }
    use pulldown_cmark::{html, Options, Parser};
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_FOOTNOTES);
    let parser = Parser::new_ext(text, options);
    let mut html_out = String::new();
    html::push_html(&mut html_out, parser);
    Ok(Value::String(html_out))
}

/// Zero-pads an integer to 3 digits, e.g. `7` -> `"007"`.
fn pad3_filter(value: &Value, _args: &HashMap<String, Value>) -> tera::Result<Value> {
    let n = value.as_i64().ok_or_else(|| tera::Error::msg("pad3: not a number"))?;
    Ok(Value::String(format!("{n:03}")))
}

/// Capitalizes the first letter of each word.
fn title_case_filter(value: &Value, _args: &HashMap<String, Value>) -> tera::Result<Value> {
    let text = value.as_str().unwrap_or("");
    let out: String = text
        .split(' ')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    Ok(Value::String(out))
}

/// The first 10 characters of an ISO timestamp string (its date part).
fn date10_filter(value: &Value, _args: &HashMap<String, Value>) -> tera::Result<Value> {
    let text = value.as_str().unwrap_or("");
    let head: String = text.chars().take(10).collect();
    Ok(Value::String(head))
}

/// `{{ names | dict_get(key=id, default="...") }}`: looks `key` up in an
/// object whose keys are numeric-id strings (as `serde_json` serializes an
/// integer-keyed map), coercing `key` (which arrives as a Tera number) to
/// its string form first.
fn dict_get_filter(value: &Value, args: &HashMap<String, Value>) -> tera::Result<Value> {
    let key = args.get("key").ok_or_else(|| tera::Error::msg("dict_get: missing `key`"))?;
    let key_str = match key {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => return Ok(args.get("default").cloned().unwrap_or(Value::Null)),
    };
    match value.get(&key_str) {
        Some(found) => Ok(found.clone()),
        None => Ok(args.get("default").cloned().unwrap_or(Value::Null)),
    }
}
