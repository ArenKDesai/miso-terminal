//! What the assistant may call: a [`ToolBox`] of read-only lookups, each
//! described by a JSON Schema. The same box serves the in-app assistant and
//! the MCP server.
//!
//! Inputs are checked against the schema before a tool runs ([`validate`]),
//! even though the API's strict mode already holds Claude to it: the MCP
//! server's callers are not held to anything.

use serde::Serialize;
use serde_json::Value;

use mt_data::BoxFuture;

/// One tool as Claude sees it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// A tool's answer: JSON for Claude, or an explanation of what went wrong
/// (sent back as an error result, so Claude can try something else).
pub type ToolResult = Result<Value, String>;

/// The tools, in a fixed order (the order is part of the cached prompt).
pub trait ToolBox: Send + Sync {
    fn specs(&self) -> Vec<ToolSpec>;
    fn call<'a>(&'a self, name: &'a str, input: Value) -> BoxFuture<'a, ToolResult>;
}

/// No tools (a plain conversation; tests).
pub struct NoTools;

impl ToolBox for NoTools {
    fn specs(&self) -> Vec<ToolSpec> {
        Vec::new()
    }
    fn call<'a>(&'a self, name: &'a str, _input: Value) -> BoxFuture<'a, ToolResult> {
        Box::pin(async move { Err(format!("there is no tool named {name}")) })
    }
}

/// Look the tool up and check the input, then run it.
pub async fn run(tools: &dyn ToolBox, name: &str, input: Value) -> ToolResult {
    let Some(spec) = tools.specs().into_iter().find(|s| s.name == name) else {
        return Err(format!("there is no tool named {name}"));
    };
    validate(&spec.input_schema, &input).map_err(|e| format!("invalid input: {e}"))?;
    tools.call(name, input).await
}

/// Check `value` against the subset of JSON Schema the tools use: `type`
/// (one or a list), `properties`, `required`, `additionalProperties: false`,
/// `items`, `enum`, `minimum`, `maximum`, `minItems`, `maxItems` and
/// `maxLength`. Anything else in a schema is ignored.
pub fn validate(schema: &Value, value: &Value) -> Result<(), String> {
    check(schema, value, "input")
}

fn check(schema: &Value, value: &Value, at: &str) -> Result<(), String> {
    if let Some(t) = schema.get("type") {
        let types: Vec<&str> = match t {
            Value::String(s) => vec![s.as_str()],
            Value::Array(a) => a.iter().filter_map(Value::as_str).collect(),
            _ => Vec::new(),
        };
        if !types.is_empty() && !types.iter().any(|t| is_type(value, t)) {
            return Err(format!("{at} should be {}", types.join(" or ")));
        }
    }
    if let Some(options) = schema.get("enum").and_then(Value::as_array)
        && !options.contains(value)
    {
        return Err(format!(
            "{at} should be one of {}",
            Value::from(options.clone())
        ));
    }
    if let Some(n) = value.as_f64() {
        if let Some(min) = schema.get("minimum").and_then(Value::as_f64)
            && n < min
        {
            return Err(format!("{at} should be at least {min}"));
        }
        if let Some(max) = schema.get("maximum").and_then(Value::as_f64)
            && n > max
        {
            return Err(format!("{at} should be at most {max}"));
        }
    }
    if let (Some(s), Some(max)) = (
        value.as_str(),
        schema.get("maxLength").and_then(Value::as_u64),
    ) && s.chars().count() as u64 > max
    {
        return Err(format!("{at} should be at most {max} characters"));
    }
    if let Some(items) = value.as_array() {
        if let Some(min) = schema.get("minItems").and_then(Value::as_u64)
            && (items.len() as u64) < min
        {
            return Err(format!("{at} needs at least {min} items"));
        }
        if let Some(max) = schema.get("maxItems").and_then(Value::as_u64)
            && items.len() as u64 > max
        {
            return Err(format!("{at} takes at most {max} items"));
        }
        if let Some(item_schema) = schema.get("items") {
            for (i, item) in items.iter().enumerate() {
                check(item_schema, item, &format!("{at}[{i}]"))?;
            }
        }
    }
    if let Some(obj) = value.as_object() {
        let props = schema.get("properties").and_then(Value::as_object);
        if let Some(required) = schema.get("required").and_then(Value::as_array) {
            for name in required.iter().filter_map(Value::as_str) {
                if !obj.contains_key(name) {
                    return Err(format!("{at} is missing {name}"));
                }
            }
        }
        for (k, v) in obj {
            match props.and_then(|p| p.get(k)) {
                Some(s) => check(s, v, &format!("{at}.{k}"))?,
                None if schema.get("additionalProperties") == Some(&Value::Bool(false)) => {
                    return Err(format!("{at} has an unknown field {k}"));
                }
                None => {}
            }
        }
    }
    Ok(())
}

fn is_type(v: &Value, t: &str) -> bool {
    match t {
        "object" => v.is_object(),
        "array" => v.is_array(),
        "string" => v.is_string(),
        "number" => v.is_number(),
        "integer" => v.is_i64() || v.is_u64(),
        "boolean" => v.is_boolean(),
        "null" => v.is_null(),
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "node": {"type": "string", "maxLength": 8},
                "days": {"type": "integer", "minimum": 1, "maximum": 31},
                "market": {"type": "string", "enum": ["da", "rt"]},
                "nodes": {"type": "array", "items": {"type": "string"}, "maxItems": 2},
                "note": {"type": ["string", "null"]}
            },
            "required": ["node"],
            "additionalProperties": false
        })
    }

    #[test]
    fn accepts_what_the_schema_allows() {
        let ok = json!({"node": "MINN.HUB", "days": 7, "market": "rt",
            "nodes": ["A", "B"], "note": null});
        assert_eq!(validate(&schema(), &ok), Ok(()));
    }

    #[test]
    fn names_what_is_wrong() {
        let cases = [
            (json!({}), "input is missing node"),
            (json!({"node": 1}), "input.node should be string"),
            (
                json!({"node": "A", "days": 0}),
                "input.days should be at least 1",
            ),
            (
                json!({"node": "A", "days": 2.5}),
                "input.days should be integer",
            ),
            (
                json!({"node": "A", "market": "x"}),
                "input.market should be one of [\"da\",\"rt\"]",
            ),
            (
                json!({"node": "A", "nodes": [1]}),
                "input.nodes[0] should be string",
            ),
            (
                json!({"node": "A", "nodes": ["a", "b", "c"]}),
                "input.nodes takes at most 2 items",
            ),
            (json!({"node": "A", "x": 1}), "input has an unknown field x"),
            (
                json!({"node": "TOO.LONG.NAME"}),
                "input.node should be at most 8 characters",
            ),
            (json!([]), "input should be object"),
        ];
        for (input, want) in cases {
            assert_eq!(validate(&schema(), &input), Err(want.to_owned()), "{input}");
        }
    }
}
