use crate::config::{FilterOperator, PayloadFilter};
use crate::models::trigger_attempt::TriggerAttemptStatus;
use crate::payload::resolve_field;

use super::EvalOutcome;

pub fn evaluate(filters: &[PayloadFilter], payload: &serde_json::Value) -> EvalOutcome {
    for filter in filters {
        if let Some(reason) = check_filter(filter, payload) {
            return EvalOutcome::Reject {
                status: TriggerAttemptStatus::Filtered,
                reason,
            };
        }
    }
    EvalOutcome::Allow
}

/// Returns `Some(reason)` if the filter fails, `None` if it passes.
fn check_filter(filter: &PayloadFilter, payload: &serde_json::Value) -> Option<String> {
    match filter.operator {
        FilterOperator::Exists => {
            let val = resolve_field(payload, &filter.field);
            match val {
                Some(v) if !v.is_null() => None,
                _ => Some(format!(
                    "field '{}' does not exist or is null",
                    filter.field
                )),
            }
        }
        FilterOperator::Equals => {
            let field_val = resolve_field(payload, &filter.field);
            let expected = filter.value.as_deref().unwrap_or("");
            match field_val {
                None => Some(format!("field '{}' not found", filter.field)),
                Some(v) => {
                    if json_as_str(v) == expected {
                        None
                    } else {
                        Some(format!(
                            "field '{}' = {} does not equal '{}'",
                            filter.field, v, expected
                        ))
                    }
                }
            }
        }
        FilterOperator::NotEquals => {
            let field_val = resolve_field(payload, &filter.field);
            let expected = filter.value.as_deref().unwrap_or("");
            match field_val {
                None => Some(format!("field '{}' not found", filter.field)),
                Some(v) => {
                    if json_as_str(v) != expected {
                        None
                    } else {
                        Some(format!(
                            "field '{}' = {} equals '{}' (not_equals failed)",
                            filter.field, v, expected
                        ))
                    }
                }
            }
        }
        FilterOperator::Contains => {
            let expected = filter.value.as_deref().unwrap_or("");
            match resolve_field(payload, &filter.field) {
                None => Some(format!("field '{}' not found", filter.field)),
                Some(serde_json::Value::String(s)) => {
                    if s.contains(expected) {
                        None
                    } else {
                        Some(format!(
                            "field '{}' = '{}' does not contain '{}'",
                            filter.field, s, expected
                        ))
                    }
                }
                Some(serde_json::Value::Array(arr)) => {
                    let found = arr.iter().any(|el| json_as_str(el) == expected);
                    if found {
                        None
                    } else {
                        Some(format!(
                            "field '{}' array does not contain element '{}'",
                            filter.field, expected
                        ))
                    }
                }
                Some(v) => Some(format!(
                    "field '{}' is {} (contains requires string or array)",
                    filter.field,
                    v.type_name()
                )),
            }
        }
        FilterOperator::Regex => {
            let pattern = filter.value.as_deref().unwrap_or("");
            // Pattern validity is guaranteed by config validation; unwrap is safe here.
            let re = regex::Regex::new(pattern).unwrap_or_else(|_| regex::Regex::new("").unwrap());
            match resolve_field(payload, &filter.field) {
                None => Some(format!("field '{}' not found", filter.field)),
                Some(v) => {
                    let s = json_as_str(v);
                    if re.is_match(&s) {
                        None
                    } else {
                        Some(format!(
                            "field '{}' = '{}' does not match regex '{}'",
                            filter.field, s, pattern
                        ))
                    }
                }
            }
        }
        FilterOperator::Gt => numeric_compare(filter, payload, |a, b| a > b, ">"),
        FilterOperator::Lt => numeric_compare(filter, payload, |a, b| a < b, "<"),
        FilterOperator::Gte => numeric_compare(filter, payload, |a, b| a >= b, ">="),
        FilterOperator::Lte => numeric_compare(filter, payload, |a, b| a <= b, "<="),
    }
}

fn numeric_compare(
    filter: &PayloadFilter,
    payload: &serde_json::Value,
    cmp: impl Fn(f64, f64) -> bool,
    op: &str,
) -> Option<String> {
    let threshold_str = filter.value.as_deref().unwrap_or("");
    let threshold: f64 = match threshold_str.parse() {
        Ok(v) => v,
        Err(_) => {
            return Some(format!(
                "filter value '{}' is not a number (required for {} operator)",
                threshold_str, op
            ));
        }
    };

    match resolve_field(payload, &filter.field) {
        None => Some(format!("field '{}' not found", filter.field)),
        Some(v) => match v.as_f64() {
            Some(field_val) => {
                if cmp(field_val, threshold) {
                    None
                } else {
                    Some(format!(
                        "field '{}' = {} is not {} {}",
                        filter.field, field_val, op, threshold
                    ))
                }
            }
            None => Some(format!(
                "field '{}' = {} is not numeric (required for {} operator)",
                filter.field, v, op
            )),
        },
    }
}

/// Return the string representation of a JSON value suitable for equality comparison.
/// Strings are returned unquoted; other values use their JSON display form.
fn json_as_str(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

trait TypeName {
    fn type_name(&self) -> &'static str;
}

impl TypeName for serde_json::Value {
    fn type_name(&self) -> &'static str {
        match self {
            serde_json::Value::Null => "null",
            serde_json::Value::Bool(_) => "boolean",
            serde_json::Value::Number(_) => "number",
            serde_json::Value::String(_) => "string",
            serde_json::Value::Array(_) => "array",
            serde_json::Value::Object(_) => "object",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PayloadFilter;
    use serde_json::json;

    fn filter(field: &str, operator: FilterOperator, value: Option<&str>) -> PayloadFilter {
        PayloadFilter {
            field: field.to_string(),
            operator,
            value: value.map(str::to_string),
        }
    }

    #[test]
    fn equals_string_field_passes() {
        let payload = json!({"action": "released"});
        let result = evaluate(
            &[filter("action", FilterOperator::Equals, Some("released"))],
            &payload,
        );
        assert!(matches!(result, EvalOutcome::Allow));
    }

    #[test]
    fn equals_string_field_rejects() {
        let payload = json!({"action": "push"});
        let result = evaluate(
            &[filter("action", FilterOperator::Equals, Some("released"))],
            &payload,
        );
        assert!(matches!(result, EvalOutcome::Reject { .. }));
    }

    #[test]
    fn equals_boolean_field() {
        let payload = json!({"draft": false});
        let result = evaluate(
            &[filter("draft", FilterOperator::Equals, Some("false"))],
            &payload,
        );
        assert!(matches!(result, EvalOutcome::Allow));
    }

    #[test]
    fn equals_number_field() {
        let payload = json!({"count": 42});
        let result = evaluate(
            &[filter("count", FilterOperator::Equals, Some("42"))],
            &payload,
        );
        assert!(matches!(result, EvalOutcome::Allow));
    }

    #[test]
    fn not_equals_passes() {
        let payload = json!({"action": "released"});
        let result = evaluate(
            &[filter("action", FilterOperator::NotEquals, Some("push"))],
            &payload,
        );
        assert!(matches!(result, EvalOutcome::Allow));
    }

    #[test]
    fn not_equals_rejects() {
        let payload = json!({"action": "push"});
        let result = evaluate(
            &[filter("action", FilterOperator::NotEquals, Some("push"))],
            &payload,
        );
        assert!(matches!(result, EvalOutcome::Reject { .. }));
    }

    #[test]
    fn contains_substring() {
        let payload = json!({"message": "deploy to prod"});
        let result = evaluate(
            &[filter("message", FilterOperator::Contains, Some("deploy"))],
            &payload,
        );
        assert!(matches!(result, EvalOutcome::Allow));
    }

    #[test]
    fn contains_array_element() {
        let payload = json!({"labels": ["deploy", "release"]});
        let result = evaluate(
            &[filter("labels", FilterOperator::Contains, Some("deploy"))],
            &payload,
        );
        assert!(matches!(result, EvalOutcome::Allow));
    }

    #[test]
    fn contains_rejects_on_type_mismatch() {
        let payload = json!({"count": 42});
        let result = evaluate(
            &[filter("count", FilterOperator::Contains, Some("4"))],
            &payload,
        );
        assert!(matches!(result, EvalOutcome::Reject { .. }));
    }

    #[test]
    fn regex_matches() {
        let payload = json!({"branch": "main"});
        let result = evaluate(
            &[filter("branch", FilterOperator::Regex, Some("^main$"))],
            &payload,
        );
        assert!(matches!(result, EvalOutcome::Allow));
    }

    #[test]
    fn regex_no_match() {
        let payload = json!({"branch": "develop"});
        let result = evaluate(
            &[filter("branch", FilterOperator::Regex, Some("^main$"))],
            &payload,
        );
        assert!(matches!(result, EvalOutcome::Reject { .. }));
    }

    #[test]
    fn exists_passes() {
        let payload = json!({"action": "any"});
        let result = evaluate(&[filter("action", FilterOperator::Exists, None)], &payload);
        assert!(matches!(result, EvalOutcome::Allow));
    }

    #[test]
    fn exists_rejects_missing() {
        let payload = json!({});
        let result = evaluate(&[filter("action", FilterOperator::Exists, None)], &payload);
        assert!(matches!(result, EvalOutcome::Reject { .. }));
    }

    #[test]
    fn exists_rejects_null() {
        let payload = json!({"action": null});
        let result = evaluate(&[filter("action", FilterOperator::Exists, None)], &payload);
        assert!(matches!(result, EvalOutcome::Reject { .. }));
    }

    #[test]
    fn gt_passes() {
        let payload = json!({"count": 10});
        let result = evaluate(&[filter("count", FilterOperator::Gt, Some("5"))], &payload);
        assert!(matches!(result, EvalOutcome::Allow));
    }

    #[test]
    fn gt_rejects() {
        let payload = json!({"count": 3});
        let result = evaluate(&[filter("count", FilterOperator::Gt, Some("5"))], &payload);
        assert!(matches!(result, EvalOutcome::Reject { .. }));
    }

    #[test]
    fn lt_gte_lte_basic() {
        let payload = json!({"count": 5});

        assert!(matches!(
            evaluate(&[filter("count", FilterOperator::Lt, Some("10"))], &payload),
            EvalOutcome::Allow
        ));
        assert!(matches!(
            evaluate(&[filter("count", FilterOperator::Lt, Some("5"))], &payload),
            EvalOutcome::Reject { .. }
        ));

        assert!(matches!(
            evaluate(&[filter("count", FilterOperator::Gte, Some("5"))], &payload),
            EvalOutcome::Allow
        ));
        assert!(matches!(
            evaluate(&[filter("count", FilterOperator::Gte, Some("6"))], &payload),
            EvalOutcome::Reject { .. }
        ));

        assert!(matches!(
            evaluate(&[filter("count", FilterOperator::Lte, Some("5"))], &payload),
            EvalOutcome::Allow
        ));
        assert!(matches!(
            evaluate(&[filter("count", FilterOperator::Lte, Some("4"))], &payload),
            EvalOutcome::Reject { .. }
        ));
    }

    #[test]
    fn numeric_comparison_rejects_non_number() {
        let payload = json!({"name": "hello"});
        let result = evaluate(&[filter("name", FilterOperator::Gt, Some("5"))], &payload);
        assert!(matches!(result, EvalOutcome::Reject { .. }));
    }

    #[test]
    fn dot_notation_nested_field() {
        let payload = json!({"repo": {"name": "myapp"}});
        let result = evaluate(
            &[filter("repo.name", FilterOperator::Equals, Some("myapp"))],
            &payload,
        );
        assert!(matches!(result, EvalOutcome::Allow));
    }

    #[test]
    fn missing_field_with_equals_rejects() {
        let payload = json!({});
        let result = evaluate(
            &[filter("action", FilterOperator::Equals, Some("push"))],
            &payload,
        );
        assert!(matches!(result, EvalOutcome::Reject { .. }));
    }

    #[test]
    fn multiple_filters_all_pass() {
        let payload = json!({"action": "released", "draft": false});
        let result = evaluate(
            &[
                filter("action", FilterOperator::Equals, Some("released")),
                filter("draft", FilterOperator::Equals, Some("false")),
            ],
            &payload,
        );
        assert!(matches!(result, EvalOutcome::Allow));
    }

    #[test]
    fn multiple_filters_first_fails() {
        let payload = json!({"action": "push", "draft": false});
        let result = evaluate(
            &[
                filter("action", FilterOperator::Equals, Some("released")),
                filter("draft", FilterOperator::Equals, Some("false")),
            ],
            &payload,
        );
        assert!(matches!(result, EvalOutcome::Reject { .. }));
    }

    #[test]
    fn empty_filters_allows() {
        let payload = json!({});
        let result = evaluate(&[], &payload);
        assert!(matches!(result, EvalOutcome::Allow));
    }
}
