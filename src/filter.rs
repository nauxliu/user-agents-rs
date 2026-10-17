use regex::Regex;
use serde_json::{Map, Value};

/// Recursive filters. Arrays and object fields are combined with AND, as upstream.
///
/// At the root, strings and regexes match `userAgent`. Within objects they match
/// the selected property. Missing properties never match, including null filters.
#[derive(Clone, Debug)]
pub enum Filter {
    Equals(Value),
    Regex(Regex),
    All(Vec<Filter>),
    Object(Vec<(String, Filter)>),
}

impl Filter {
    /// Convert JSON objects/arrays recursively; primitive values use equality.
    pub fn from_json(value: Value) -> Self {
        match value {
            Value::Object(fields) => Self::object(fields.into_iter().map(|(k, v)| (k, v.into()))),
            Value::Array(filters) => Self::All(filters.into_iter().map(Self::from_json).collect()),
            other => Self::Equals(other),
        }
    }

    pub fn regex(pattern: &str) -> Result<Self, regex::Error> {
        Regex::new(pattern).map(Self::Regex)
    }

    pub fn object(fields: impl IntoIterator<Item = (impl Into<String>, Filter)>) -> Self {
        Self::Object(fields.into_iter().map(|(k, v)| (k.into(), v)).collect())
    }

    pub fn matches(&self, value: &Value) -> bool {
        match self {
            Self::Equals(expected) => equal(expected, ua_or_value(value)),
            Self::Regex(regex) => ua_or_value(value)
                .as_str()
                .is_some_and(|s| regex.is_match(s)),
            Self::All(filters) => filters.iter().all(|filter| filter.matches(value)),
            Self::Object(fields) => fields
                .iter()
                .all(|(key, filter)| value.get(key).is_some_and(|child| filter.matches(child))),
        }
    }
}

fn ua_or_value(value: &Value) -> &Value {
    value
        .get("userAgent")
        .filter(|ua| ua.as_str().is_some_and(|s| !s.is_empty()))
        .unwrap_or(value)
}

// Compare integral JSON numbers exactly, while allowing 50 to equal 50.0.
// Converting every u64 to f64 would merge distinct values above 2^53.
fn equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(a), Value::Number(b)) if a != b => {
            if a.is_f64() && !b.is_f64() {
                float_equals_integer(a.as_f64().unwrap(), b)
            } else if b.is_f64() && !a.is_f64() {
                float_equals_integer(b.as_f64().unwrap(), a)
            } else {
                false
            }
        }
        _ => a == b,
    }
}

fn float_equals_integer(float: f64, integer: &serde_json::Number) -> bool {
    if float.fract() != 0.0 {
        return false;
    }
    // Explicit exclusive upper bounds avoid Rust's saturating float casts at
    // u64::MAX/i64::MAX, which round to the next power of two as f64.
    if let Some(value) = integer.as_u64() {
        (0.0..18_446_744_073_709_551_616.0).contains(&float) && float as u64 == value
    } else if let Some(value) = integer.as_i64() {
        (-9_223_372_036_854_775_808.0..0.0).contains(&float) && float as i64 == value
    } else {
        false
    }
}

impl From<Value> for Filter {
    fn from(value: Value) -> Self {
        Self::from_json(value)
    }
}

impl From<&str> for Filter {
    fn from(value: &str) -> Self {
        Self::Equals(Value::String(value.into()))
    }
}

impl From<Regex> for Filter {
    fn from(value: Regex) -> Self {
        Self::Regex(value)
    }
}

impl From<Map<String, Value>> for Filter {
    fn from(value: Map<String, Value>) -> Self {
        Self::from_json(Value::Object(value))
    }
}
