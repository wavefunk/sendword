use std::fmt;

use serde::{Deserialize, Serialize};

/// The JSON type a payload field is expected to have.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FieldType {
    String,
    Number,
    Boolean,
    Object,
    Array,
}

impl FieldType {
    /// Returns true if the JSON value matches this type.
    pub fn matches_value(self, value: &serde_json::Value) -> bool {
        match self {
            Self::String => value.is_string(),
            Self::Number => value.is_number(),
            Self::Boolean => value.is_boolean(),
            Self::Object => value.is_object(),
            Self::Array => value.is_array(),
        }
    }
}

impl fmt::Display for FieldType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::String => f.write_str("string"),
            Self::Number => f.write_str("number"),
            Self::Boolean => f.write_str("boolean"),
            Self::Object => f.write_str("object"),
            Self::Array => f.write_str("array"),
        }
    }
}

/// A single field definition within a payload schema.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PayloadField {
    pub name: String,
    #[serde(rename = "type")]
    pub field_type: FieldType,
    #[serde(default)]
    pub required: bool,
}

/// Per-hook payload schema: a list of expected fields.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PayloadSchema {
    pub fields: Vec<PayloadField>,
}

/// A single validation error for a payload field.
#[derive(Debug, Serialize)]
pub struct FieldValidationError {
    pub field: String,
    pub message: String,
}

/// Walk a JSON value by dot-separated path (e.g., "repo.name").
/// Returns None if any intermediate segment is missing or not an object.
pub fn resolve_field<'a>(root: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value> {
    let mut current = root;
    for segment in path.split('.') {
        current = current.get(segment)?;
    }
    Some(current)
}

impl PayloadSchema {
    /// Validate a JSON value against this schema.
    ///
    /// Returns `Ok(())` if all required fields are present and all present
    /// fields match their declared types. Returns `Err` with a list of all
    /// validation errors (not short-circuited).
    pub fn validate(&self, payload: &serde_json::Value) -> Result<(), Vec<FieldValidationError>> {
        let mut errors = Vec::new();

        for field in &self.fields {
            match resolve_field(payload, &field.name) {
                None | Some(serde_json::Value::Null) => {
                    if field.required {
                        errors.push(FieldValidationError {
                            field: field.name.clone(),
                            message: format!(
                                "required field '{}' is missing",
                                field.name,
                            ),
                        });
                    }
                }
                Some(value) => {
                    if !field.field_type.matches_value(value) {
                        errors.push(FieldValidationError {
                            field: field.name.clone(),
                            message: format!(
                                "field '{}' expected type {}, got {}",
                                field.name,
                                field.field_type,
                                json_type_name(value),
                            ),
                        });
                    }
                }
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

/// Return the JSON type name for a value (for error messages).
fn json_type_name(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn schema(fields: Vec<PayloadField>) -> PayloadSchema {
        PayloadSchema { fields }
    }

    fn field(name: &str, ft: FieldType, required: bool) -> PayloadField {
        PayloadField {
            name: name.to_owned(),
            field_type: ft,
            required,
        }
    }

    #[test]
    fn valid_payload_with_all_required_fields() {
        let s = schema(vec![
            field("action", FieldType::String, true),
            field("count", FieldType::Number, true),
        ]);
        let payload = json!({"action": "deploy", "count": 3});
        assert!(s.validate(&payload).is_ok());
    }

    #[test]
    fn missing_required_field_returns_error() {
        let s = schema(vec![field("action", FieldType::String, true)]);
        let payload = json!({});
        let errors = s.validate(&payload).unwrap_err();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].field, "action");
        assert!(errors[0].message.contains("missing"));
    }

    #[test]
    fn missing_optional_field_is_ok() {
        let s = schema(vec![field("tag", FieldType::String, false)]);
        let payload = json!({});
        assert!(s.validate(&payload).is_ok());
    }

    #[test]
    fn type_mismatch_returns_error() {
        let s = schema(vec![field("count", FieldType::Number, true)]);
        let payload = json!({"count": "not-a-number"});
        let errors = s.validate(&payload).unwrap_err();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("expected type number"));
        assert!(errors[0].message.contains("got string"));
    }

    #[test]
    fn null_value_treated_as_missing() {
        let s = schema(vec![field("action", FieldType::String, true)]);
        let payload = json!({"action": null});
        let errors = s.validate(&payload).unwrap_err();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("missing"));
    }

    #[test]
    fn dot_notation_traverses_nested_objects() {
        let s = schema(vec![field("repo.name", FieldType::String, true)]);
        let payload = json!({"repo": {"name": "myapp"}});
        assert!(s.validate(&payload).is_ok());
    }

    #[test]
    fn dot_notation_missing_parent_returns_error() {
        let s = schema(vec![field("repo.name", FieldType::String, true)]);
        let payload = json!({});
        let errors = s.validate(&payload).unwrap_err();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("missing"));
    }

    #[test]
    fn dot_notation_parent_not_object_returns_error() {
        let s = schema(vec![field("repo.name", FieldType::String, true)]);
        let payload = json!({"repo": "not-an-object"});
        let errors = s.validate(&payload).unwrap_err();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("missing"));
    }

    #[test]
    fn all_field_types_match_correctly() {
        // String
        assert!(FieldType::String.matches_value(&json!("hello")));
        assert!(!FieldType::String.matches_value(&json!(42)));

        // Number
        assert!(FieldType::Number.matches_value(&json!(42)));
        assert!(FieldType::Number.matches_value(&json!(3.14)));
        assert!(!FieldType::Number.matches_value(&json!("42")));

        // Boolean
        assert!(FieldType::Boolean.matches_value(&json!(true)));
        assert!(!FieldType::Boolean.matches_value(&json!(1)));

        // Object
        assert!(FieldType::Object.matches_value(&json!({})));
        assert!(!FieldType::Object.matches_value(&json!([])));

        // Array
        assert!(FieldType::Array.matches_value(&json!([])));
        assert!(!FieldType::Array.matches_value(&json!({})));
    }

    #[test]
    fn multiple_errors_accumulated() {
        let s = schema(vec![
            field("a", FieldType::String, true),
            field("b", FieldType::Number, true),
            field("c", FieldType::Boolean, true),
        ]);
        let payload = json!({});
        let errors = s.validate(&payload).unwrap_err();
        assert_eq!(errors.len(), 3);
    }

    #[test]
    fn empty_schema_accepts_anything() {
        let s = schema(vec![]);
        assert!(s.validate(&json!({})).is_ok());
        assert!(s.validate(&json!({"anything": "goes"})).is_ok());
    }

    #[test]
    fn present_optional_field_still_type_checked() {
        let s = schema(vec![field("tag", FieldType::String, false)]);
        let payload = json!({"tag": 123});
        let errors = s.validate(&payload).unwrap_err();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("expected type string"));
    }

    #[test]
    fn field_type_display_formatting() {
        assert_eq!(FieldType::String.to_string(), "string");
        assert_eq!(FieldType::Number.to_string(), "number");
        assert_eq!(FieldType::Boolean.to_string(), "boolean");
        assert_eq!(FieldType::Object.to_string(), "object");
        assert_eq!(FieldType::Array.to_string(), "array");
    }

    #[test]
    fn deserialize_field_type_from_lowercase() {
        let ft: FieldType = serde_json::from_str("\"string\"").unwrap();
        assert_eq!(ft, FieldType::String);
        let ft: FieldType = serde_json::from_str("\"number\"").unwrap();
        assert_eq!(ft, FieldType::Number);
    }
}
