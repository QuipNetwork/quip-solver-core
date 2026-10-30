//! A validator for the small JSON Schema subset that `schema.json` uses.
//!
//! The subset is: annotations (`$schema`, `$id`, `$comment`, `title`,
//! `description`), `$defs`, `$ref` to `#/$defs/<name>`, `type`, `properties`,
//! `required`, `additionalProperties: false`, `items`, `minItems`, `maxItems`,
//! `enum`, `const`, `minLength`, `maxLength`, `minimum`, `maximum`,
//! `contentEncoding: "base16"`, and the two `pattern` strings for decimal
//! `u64` and `i64` values. `check_keywords` rejects everything else, so a new
//! keyword cannot pass through this validator unchecked.

use serde_json::{Map, Value};

const U64_PATTERN: &str = "^(0|[1-9][0-9]*)$";
const I64_PATTERN: &str = "^(0|-?[1-9][0-9]*)$";
const TYPE_NAMES: [&str; 6] = ["object", "array", "string", "integer", "boolean", "null"];
const DEFS_PREFIX: &str = "#/$defs/";

type Check = Result<(), String>;

/// Rejects any keyword or keyword value in `schema` that is outside the subset.
pub(crate) fn check_keywords(schema: &Value) -> Check {
    let root = schema
        .as_object()
        .ok_or("the schema root is not an object")?;
    let defs = root.get("$defs").and_then(Value::as_object);
    walk_schema(schema, "", defs)
}

/// Validates `doc` against the root schema. Stops at the first failure.
pub(crate) fn validate(schema: &Value, doc: &Value) -> Check {
    check_node(schema, schema, doc, "")
}

/// Validates `value` against the named entry of `$defs`.
pub(crate) fn validate_def(schema: &Value, def: &str, value: &Value) -> Check {
    let node = lookup_def(schema, def)?;
    check_node(schema, node, value, "")
}

fn lookup_def<'a>(root: &'a Value, name: &str) -> Result<&'a Value, String> {
    root.get("$defs")
        .and_then(|d| d.get(name))
        .ok_or_else(|| format!("no $defs entry named {name}"))
}

fn resolve_ref<'a>(root: &'a Value, target: &str) -> Result<&'a Value, String> {
    let name = target
        .strip_prefix(DEFS_PREFIX)
        .ok_or_else(|| format!("unsupported $ref {target}"))?;
    lookup_def(root, name)
}

fn walk_schema(node: &Value, at: &str, defs: Option<&Map<String, Value>>) -> Check {
    let map = node
        .as_object()
        .ok_or_else(|| format!("{at}: a schema must be an object"))?;
    for (key, value) in map {
        let here = format!("{at}/{key}");
        match key.as_str() {
            "$schema" | "$id" | "$comment" | "title" | "description" => {
                if !value.is_string() {
                    return Err(format!("{here}: annotation must be a string"));
                }
            }
            "$defs" => {
                let entries = value
                    .as_object()
                    .ok_or_else(|| format!("{here}: must be an object"))?;
                for (name, def) in entries {
                    walk_schema(def, &format!("{here}/{name}"), defs)?;
                }
            }
            "$ref" => {
                let target = value
                    .as_str()
                    .and_then(|t| t.strip_prefix(DEFS_PREFIX))
                    .ok_or_else(|| format!("{here}: only #/$defs/<name> is supported"))?;
                if !defs.is_some_and(|d| d.contains_key(target)) {
                    return Err(format!("{here}: no $defs entry named {target}"));
                }
            }
            "type" => check_type_value(value, &here)?,
            "properties" => {
                let props = value
                    .as_object()
                    .ok_or_else(|| format!("{here}: must be an object"))?;
                for (name, sub) in props {
                    walk_schema(sub, &format!("{here}/{name}"), defs)?;
                }
            }
            "required" => {
                let names = value
                    .as_array()
                    .filter(|a| a.iter().all(Value::is_string))
                    .ok_or_else(|| format!("{here}: must be an array of strings"))?;
                let props = map.get("properties").and_then(Value::as_object);
                for name in names.iter().filter_map(Value::as_str) {
                    if !props.is_some_and(|p| p.contains_key(name)) {
                        return Err(format!("{here}: {name} is not in properties"));
                    }
                }
            }
            "additionalProperties" => {
                if value != &Value::Bool(false) {
                    return Err(format!("{here}: only false is supported"));
                }
            }
            "items" => walk_schema(value, &here, defs)?,
            "minItems" | "maxItems" | "minLength" | "maxLength" => {
                if value.as_u64().is_none() {
                    return Err(format!("{here}: must be a non-negative integer"));
                }
            }
            "minimum" | "maximum" => {
                if value.as_i64().is_none() {
                    return Err(format!("{here}: must be an integer"));
                }
            }
            "enum" => {
                if value.as_array().is_none_or(Vec::is_empty) {
                    return Err(format!("{here}: must be a non-empty array"));
                }
            }
            "const" => {}
            "contentEncoding" => {
                if value != "base16" {
                    return Err(format!("{here}: only \"base16\" is supported"));
                }
            }
            "pattern" => {
                if value != U64_PATTERN && value != I64_PATTERN {
                    return Err(format!("{here}: unsupported pattern"));
                }
            }
            other => return Err(format!("{here}: unsupported keyword {other}")),
        }
    }
    Ok(())
}

fn check_type_value(value: &Value, at: &str) -> Check {
    let names: Vec<&Value> = match value {
        Value::String(_) => vec![value],
        Value::Array(items) if !items.is_empty() => items.iter().collect(),
        _ => return Err(format!("{at}: must be a string or a non-empty array")),
    };
    for name in names {
        if !name.as_str().is_some_and(|n| TYPE_NAMES.contains(&n)) {
            return Err(format!("{at}: unsupported type {name}"));
        }
    }
    Ok(())
}

fn fail(at: &str, message: &str) -> Check {
    let pointer = if at.is_empty() { "/" } else { at };
    Err(format!("{pointer}: {message}"))
}

fn type_matches(name: &str, value: &Value) -> bool {
    match name {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "integer" => value.is_i64() || value.is_u64(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        _ => false,
    }
}

fn check_node(root: &Value, schema: &Value, value: &Value, at: &str) -> Check {
    let map = schema
        .as_object()
        .ok_or_else(|| format!("{at}: schema node is not an object"))?;
    if let Some(target) = map.get("$ref").and_then(Value::as_str) {
        check_node(root, resolve_ref(root, target)?, value, at)?;
    }
    if let Some(type_spec) = map.get("type") {
        let names: Vec<&str> = match type_spec {
            Value::Array(items) => items.iter().filter_map(Value::as_str).collect(),
            other => other.as_str().into_iter().collect(),
        };
        if !names.iter().any(|n| type_matches(n, value)) {
            return fail(at, &format!("expected type {type_spec}"));
        }
    }
    if let Some(expected) = map.get("const") {
        if expected != value {
            return fail(at, &format!("expected the constant {expected}"));
        }
    }
    if let Some(choices) = map.get("enum").and_then(Value::as_array) {
        if !choices.contains(value) {
            return fail(at, "value is not one of the allowed values");
        }
    }
    match value {
        Value::String(text) => check_string(map, text, at)?,
        Value::Number(_) => check_number(map, value, at)?,
        Value::Array(items) => check_array(root, map, items, at)?,
        Value::Object(fields) => check_object(root, map, fields, at)?,
        Value::Bool(_) | Value::Null => {}
    }
    Ok(())
}

fn check_string(map: &Map<String, Value>, text: &str, at: &str) -> Check {
    let length = text.chars().count() as u64;
    if map
        .get("minLength")
        .and_then(Value::as_u64)
        .is_some_and(|m| length < m)
    {
        return fail(at, "string is too short");
    }
    if map
        .get("maxLength")
        .and_then(Value::as_u64)
        .is_some_and(|m| length > m)
    {
        return fail(at, "string is too long");
    }
    if map.contains_key("contentEncoding")
        && !(text.len().is_multiple_of(2)
            && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
    {
        return fail(at, "not lowercase base16 of even length");
    }
    match map.get("pattern").and_then(Value::as_str) {
        Some(U64_PATTERN) if !is_decimal(text, false) => fail(at, "not a decimal u64 string"),
        Some(I64_PATTERN) if !is_decimal(text, true) => fail(at, "not a decimal i64 string"),
        _ => Ok(()),
    }
}

/// Matches `^(0|[1-9][0-9]*)$`, or with `signed`, `^(0|-?[1-9][0-9]*)$`.
fn is_decimal(text: &str, signed: bool) -> bool {
    if text == "0" {
        return true;
    }
    let digits = if signed {
        text.strip_prefix('-').unwrap_or(text)
    } else {
        text
    };
    let mut chars = digits.chars();
    matches!(chars.next(), Some('1'..='9')) && chars.all(|c| c.is_ascii_digit())
}

fn check_number(map: &Map<String, Value>, value: &Value, at: &str) -> Check {
    let Some(n) = value.as_i64() else {
        return Ok(());
    };
    if map
        .get("minimum")
        .and_then(Value::as_i64)
        .is_some_and(|m| n < m)
    {
        return fail(at, "number is below the minimum");
    }
    if map
        .get("maximum")
        .and_then(Value::as_i64)
        .is_some_and(|m| n > m)
    {
        return fail(at, "number is above the maximum");
    }
    Ok(())
}

fn check_array(root: &Value, map: &Map<String, Value>, items: &[Value], at: &str) -> Check {
    let count = items.len() as u64;
    if map
        .get("minItems")
        .and_then(Value::as_u64)
        .is_some_and(|m| count < m)
    {
        return fail(at, "array has too few items");
    }
    if map
        .get("maxItems")
        .and_then(Value::as_u64)
        .is_some_and(|m| count > m)
    {
        return fail(at, "array has too many items");
    }
    if let Some(item_schema) = map.get("items") {
        for (index, item) in items.iter().enumerate() {
            check_node(root, item_schema, item, &format!("{at}/{index}"))?;
        }
    }
    Ok(())
}

fn check_object(
    root: &Value,
    map: &Map<String, Value>,
    fields: &Map<String, Value>,
    at: &str,
) -> Check {
    if let Some(required) = map.get("required").and_then(Value::as_array) {
        for name in required.iter().filter_map(Value::as_str) {
            if !fields.contains_key(name) {
                return fail(&format!("{at}/{name}"), "required property is missing");
            }
        }
    }
    let props = map.get("properties").and_then(Value::as_object);
    for (name, field) in fields {
        let here = format!("{at}/{name}");
        match props.and_then(|p| p.get(name)) {
            Some(sub) => check_node(root, sub, field, &here)?,
            None if map.get("additionalProperties") == Some(&Value::Bool(false)) => {
                return fail(&here, "property is not allowed");
            }
            None => {}
        }
    }
    Ok(())
}
