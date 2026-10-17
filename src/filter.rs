use regex::Regex;
use serde_json::{Map, Value};
use std::sync::Arc;

/// Recursive filters. Arrays and object fields are combined with AND, as upstream.
///
/// At the root, strings and regexes match `userAgent`. Within objects they match
/// the selected property. Missing properties never match, including null filters.
#[derive(Clone)]
pub enum Filter {
    Equals(Value),
    Regex(Regex),
    All(Vec<Filter>),
    Object(Vec<(String, Filter)>),
    Predicate(Arc<dyn Fn(&Value) -> bool + Send + Sync>),
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

    pub fn predicate(f: impl Fn(&Value) -> bool + Send + Sync + 'static) -> Self {
        Self::Predicate(Arc::new(f))
    }

    pub fn matches(&self, value: &Value) -> bool {
        match self {
            Self::Equals(expected) => equal(expected, ua_or_value(value)),
            Self::Regex(regex) => regex.is_match(&js_string(ua_or_value(value))),
            Self::All(filters) => filters.iter().all(|filter| filter.matches(value)),
            Self::Object(fields) => fields
                .iter()
                .all(|(key, filter)| value.get(key).is_some_and(|child| filter.matches(child))),
            Self::Predicate(predicate) => predicate(value),
        }
    }
}

fn ua_or_value(value: &Value) -> &Value {
    value
        .get("userAgent")
        .filter(|ua| ua.as_str().is_some_and(|s| !s.is_empty()))
        .unwrap_or(value)
}

// JavaScript numbers do not distinguish integer and floating-point JSON syntax.
fn equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(a), Value::Number(b)) => a.as_f64() == b.as_f64(),
        _ => a == b,
    }
}

fn js_string(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Object(_) => "[object Object]".into(),
        Value::Array(values) => values
            .iter()
            .map(|v| {
                if v.is_null() {
                    String::new()
                } else {
                    js_string(v)
                }
            })
            .collect::<Vec<_>>()
            .join(","),
        other => other.to_string(),
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
