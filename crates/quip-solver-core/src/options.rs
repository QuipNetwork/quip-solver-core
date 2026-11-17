//! Per-algorithm options for `--solve`.
//!
//! A solver declares the options it accepts with
//! [`Sampler::solve_options`](crate::Sampler::solve_options). The `--solve`
//! input carries values in an `options` object. The SDK checks that object
//! against the declaration before the sampler runs, so a sampler receives only
//! declared names with values of the declared type.

use quip_proto::v1::OptionType;
use serde_json::{Map, Value};
use std::collections::BTreeMap;

/// One option a solver accepts: its name and the scalar type of its value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OptionDecl {
    /// The key in the `--solve` `options` object.
    pub name: &'static str,
    /// The type the value must have.
    pub kind: OptionType,
}

/// A validated option value.
#[derive(Clone, Debug, PartialEq)]
pub enum OptionValue {
    /// A JSON boolean, for an [`OptionType::Bool`] option.
    Bool(bool),
    /// A JSON integer that fits `i64`, for an [`OptionType::Int`] option.
    Int(i64),
    /// A JSON number, for an [`OptionType::Number`] option. An integer is a
    /// number.
    Number(f64),
    /// A JSON string, for an [`OptionType::String`] option.
    Text(String),
}

/// The validated options of one solve, by name. Empty when the request sent
/// none, and for every job the session serves.
pub type SolveOptions = BTreeMap<String, OptionValue>;

/// Check `raw` against `declared` and convert each value.
///
/// # Errors
///
/// Returns a message naming the first option that is not declared, or whose
/// value does not have the declared type.
pub(crate) fn validate(
    raw: &Map<String, Value>,
    declared: &[OptionDecl],
) -> Result<SolveOptions, String> {
    raw.iter()
        .map(|(name, value)| {
            let decl = declared
                .iter()
                .find(|d| d.name == name)
                .ok_or_else(|| format!("option \"{name}\" is not one this solver accepts"))?;
            let typed = match (decl.kind, value) {
                (OptionType::Bool, Value::Bool(b)) => Some(OptionValue::Bool(*b)),
                (OptionType::Int, Value::Number(n)) => n.as_i64().map(OptionValue::Int),
                (OptionType::Number, Value::Number(n)) => n.as_f64().map(OptionValue::Number),
                (OptionType::String, Value::String(s)) => Some(OptionValue::Text(s.clone())),
                _ => None,
            };
            typed.map(|v| (name.clone(), v)).ok_or_else(|| {
                format!(
                    "option \"{name}\" needs a value of type {}",
                    decl.kind.as_str_name()
                )
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const DECLARED: &[OptionDecl] = &[
        OptionDecl {
            name: "flag",
            kind: OptionType::Bool,
        },
        OptionDecl {
            name: "count",
            kind: OptionType::Int,
        },
        OptionDecl {
            name: "rate",
            kind: OptionType::Number,
        },
        OptionDecl {
            name: "mode",
            kind: OptionType::String,
        },
    ];

    fn check(value: &Value) -> Result<SolveOptions, String> {
        validate(value.as_object().expect("object"), DECLARED)
    }

    #[test]
    fn valid_values_convert_by_declared_type() {
        let got = check(&json!({"flag": true, "count": 3, "rate": 2, "mode": "fast"}))
            .expect("valid options");
        assert_eq!(*got.get("flag").expect("flag"), OptionValue::Bool(true));
        assert_eq!(*got.get("count").expect("count"), OptionValue::Int(3));
        assert_eq!(*got.get("rate").expect("rate"), OptionValue::Number(2.0));
        assert_eq!(
            *got.get("mode").expect("mode"),
            OptionValue::Text("fast".into())
        );
    }

    #[test]
    fn an_empty_object_is_valid() {
        assert!(check(&json!({})).expect("empty").is_empty());
    }

    #[test]
    fn an_undeclared_name_is_refused() {
        let err = check(&json!({"nope": 1})).expect_err("undeclared");
        assert!(err.contains("nope"), "{err}");
    }

    #[test]
    fn a_wrong_type_is_refused() {
        for bad in [
            json!({"flag": 1}),
            json!({"count": 1.5}),
            json!({"count": "3"}),
            json!({"rate": "x"}),
            json!({"mode": 4}),
            json!({"mode": null}),
            json!({"count": 18_446_744_073_709_551_615_u64}),
        ] {
            assert!(check(&bad).is_err(), "{bad} must be refused");
        }
    }
}
