use regex::Regex;
use std::borrow::Cow;
use std::sync::LazyLock;

use crate::payload::resolve_field;

/// Regex matching `{{field_name}}` placeholders in command templates.
/// Allows dotted paths like `{{repo.name}}` and trims internal whitespace
/// so `{{ repo.name }}` also works.
static PLACEHOLDER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\{\{\s*([\w.]+)\s*\}\}").expect("placeholder regex is valid"));

/// Shell-escape a string value for safe embedding in `sh -c` commands.
///
/// Wraps the value in single quotes. Any internal single quotes are escaped
/// using the POSIX idiom `'\''` (end quote, backslash-escaped literal quote,
/// start quote).
pub fn shell_escape(value: &str) -> String {
    if !value.contains('\'') {
        format!("'{value}'")
    } else {
        let escaped = value.replace('\'', "'\\''");
        format!("'{escaped}'")
    }
}

/// Convert a resolved `serde_json::Value` to its string representation
/// for interpolation.
///
/// - Strings: raw value (no JSON quotes)
/// - Numbers/booleans: their display form
/// - Arrays/objects: compact JSON
/// - Null: empty string
fn value_to_string(value: &serde_json::Value) -> Cow<'_, str> {
    match value {
        serde_json::Value::String(s) => Cow::Borrowed(s.as_str()),
        serde_json::Value::Number(n) => Cow::Owned(n.to_string()),
        serde_json::Value::Bool(b) => Cow::Borrowed(if *b { "true" } else { "false" }),
        serde_json::Value::Null => Cow::Borrowed(""),
        // Arrays and objects: compact JSON representation
        other => Cow::Owned(other.to_string()),
    }
}

/// Interpolate `{{field_name}}` placeholders in a command template.
///
/// For each placeholder found:
/// 1. Resolve the field path against the payload JSON (supports dot-notation).
/// 2. Convert the resolved value to a string.
/// 3. Shell-escape the string.
/// 4. Replace the placeholder with the escaped value.
///
/// Unresolved placeholders are left as-is (not replaced).
///
/// Returns `Cow::Borrowed` if no placeholders were found (zero allocation).
pub fn interpolate_command<'a>(template: &'a str, payload: &serde_json::Value) -> Cow<'a, str> {
    if !template.contains("{{") {
        return Cow::Borrowed(template);
    }

    let result = PLACEHOLDER_RE.replace_all(template, |caps: &regex::Captures| {
        let field_path = &caps[1];
        match resolve_field(payload, field_path) {
            Some(value) => shell_escape(&value_to_string(value)),
            None => caps[0].to_string(),
        }
    });

    match result {
        Cow::Borrowed(_) => Cow::Borrowed(template),
        Cow::Owned(s) => Cow::Owned(s),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // --- shell_escape tests ---

    #[test]
    fn shell_escape_simple_string() {
        assert_eq!(shell_escape("hello"), "'hello'");
    }

    #[test]
    fn shell_escape_string_with_spaces() {
        assert_eq!(shell_escape("hello world"), "'hello world'");
    }

    #[test]
    fn shell_escape_string_with_special_chars() {
        assert_eq!(
            shell_escape("rm -rf /; echo pwned"),
            "'rm -rf /; echo pwned'"
        );
    }

    #[test]
    fn shell_escape_string_with_single_quotes() {
        assert_eq!(shell_escape("it's"), "'it'\\''s'");
    }

    #[test]
    fn shell_escape_string_with_backticks() {
        assert_eq!(shell_escape("`whoami`"), "'`whoami`'");
    }

    #[test]
    fn shell_escape_string_with_dollar_expansion() {
        assert_eq!(shell_escape("$(cat /etc/passwd)"), "'$(cat /etc/passwd)'");
    }

    #[test]
    fn shell_escape_empty_string() {
        assert_eq!(shell_escape(""), "''");
    }

    #[test]
    fn shell_escape_string_with_newline() {
        assert_eq!(shell_escape("line1\nline2"), "'line1\nline2'");
    }

    // --- value_to_string tests ---

    #[test]
    fn value_to_string_string() {
        let v = json!("hello");
        assert_eq!(value_to_string(&v).as_ref(), "hello");
    }

    #[test]
    fn value_to_string_number_integer() {
        let v = json!(42);
        assert_eq!(value_to_string(&v).as_ref(), "42");
    }

    #[test]
    fn value_to_string_number_float() {
        let v = json!(3.14);
        assert_eq!(value_to_string(&v).as_ref(), "3.14");
    }

    #[test]
    fn value_to_string_boolean_true() {
        let v = json!(true);
        assert_eq!(value_to_string(&v).as_ref(), "true");
    }

    #[test]
    fn value_to_string_boolean_false() {
        let v = json!(false);
        assert_eq!(value_to_string(&v).as_ref(), "false");
    }

    #[test]
    fn value_to_string_null() {
        let v = json!(null);
        assert_eq!(value_to_string(&v).as_ref(), "");
    }

    #[test]
    fn value_to_string_array() {
        let v = json!([1, 2, 3]);
        assert_eq!(value_to_string(&v).as_ref(), "[1,2,3]");
    }

    #[test]
    fn value_to_string_object() {
        let v = json!({"key": "val"});
        assert_eq!(value_to_string(&v).as_ref(), r#"{"key":"val"}"#);
    }

    // --- interpolate_command tests ---

    #[test]
    fn no_placeholders_returns_borrowed() {
        let cmd = "echo hello";
        let payload = json!({});
        let result = interpolate_command(cmd, &payload);
        assert!(matches!(result, Cow::Borrowed(_)));
        assert_eq!(result.as_ref(), "echo hello");
    }

    #[test]
    fn simple_string_field_interpolated() {
        let cmd = "deploy {{action}}";
        let payload = json!({"action": "rollout"});
        let result = interpolate_command(cmd, &payload);
        assert_eq!(result.as_ref(), "deploy 'rollout'");
    }

    #[test]
    fn dot_notation_nested_field() {
        let cmd = "echo {{repo.name}}";
        let payload = json!({"repo": {"name": "myapp"}});
        let result = interpolate_command(cmd, &payload);
        assert_eq!(result.as_ref(), "echo 'myapp'");
    }

    #[test]
    fn number_field_interpolated() {
        let cmd = "scale --replicas={{count}}";
        let payload = json!({"count": 3});
        let result = interpolate_command(cmd, &payload);
        assert_eq!(result.as_ref(), "scale --replicas='3'");
    }

    #[test]
    fn boolean_field_interpolated() {
        let cmd = "deploy --dry-run={{dry_run}}";
        let payload = json!({"dry_run": true});
        let result = interpolate_command(cmd, &payload);
        assert_eq!(result.as_ref(), "deploy --dry-run='true'");
    }

    #[test]
    fn unresolved_placeholder_left_as_is() {
        let cmd = "echo {{missing_field}}";
        let payload = json!({});
        let result = interpolate_command(cmd, &payload);
        assert_eq!(result.as_ref(), "echo {{missing_field}}");
    }

    #[test]
    fn multiple_placeholders() {
        let cmd = "deploy {{app}} to {{env}}";
        let payload = json!({"app": "frontend", "env": "production"});
        let result = interpolate_command(cmd, &payload);
        assert_eq!(result.as_ref(), "deploy 'frontend' to 'production'");
    }

    #[test]
    fn injection_via_semicolon_is_escaped() {
        let cmd = "echo {{name}}";
        let payload = json!({"name": "foo; rm -rf /"});
        let result = interpolate_command(cmd, &payload);
        assert_eq!(result.as_ref(), "echo 'foo; rm -rf /'");
    }

    #[test]
    fn injection_via_backtick_is_escaped() {
        let cmd = "echo {{name}}";
        let payload = json!({"name": "`cat /etc/passwd`"});
        let result = interpolate_command(cmd, &payload);
        assert_eq!(result.as_ref(), "echo '`cat /etc/passwd`'");
    }

    #[test]
    fn injection_via_dollar_expansion_is_escaped() {
        let cmd = "echo {{name}}";
        let payload = json!({"name": "$(whoami)"});
        let result = interpolate_command(cmd, &payload);
        assert_eq!(result.as_ref(), "echo '$(whoami)'");
    }

    #[test]
    fn injection_via_single_quote_breakout_is_escaped() {
        let cmd = "echo {{name}}";
        let payload = json!({"name": "'; rm -rf / '"});
        let result = interpolate_command(cmd, &payload);
        assert_eq!(result.as_ref(), "echo ''\\''; rm -rf / '\\'''");
    }

    #[test]
    fn whitespace_around_field_name_is_trimmed() {
        let cmd = "echo {{ action }}";
        let payload = json!({"action": "deploy"});
        let result = interpolate_command(cmd, &payload);
        assert_eq!(result.as_ref(), "echo 'deploy'");
    }

    #[test]
    fn array_field_produces_json() {
        let cmd = "process {{items}}";
        let payload = json!({"items": [1, 2, 3]});
        let result = interpolate_command(cmd, &payload);
        assert_eq!(result.as_ref(), "process '[1,2,3]'");
    }

    #[test]
    fn object_field_produces_json() {
        let cmd = "process {{config}}";
        let payload = json!({"config": {"key": "val"}});
        let result = interpolate_command(cmd, &payload);
        assert_eq!(result.as_ref(), r#"process '{"key":"val"}'"#);
    }

    #[test]
    fn mixed_resolved_and_unresolved() {
        let cmd = "echo {{found}} and {{missing}}";
        let payload = json!({"found": "yes"});
        let result = interpolate_command(cmd, &payload);
        assert_eq!(result.as_ref(), "echo 'yes' and {{missing}}");
    }

    #[test]
    fn deeply_nested_dot_notation() {
        let cmd = "echo {{a.b.c}}";
        let payload = json!({"a": {"b": {"c": "deep"}}});
        let result = interpolate_command(cmd, &payload);
        assert_eq!(result.as_ref(), "echo 'deep'");
    }

    #[test]
    fn null_field_interpolates_to_empty_quoted_string() {
        let cmd = "echo {{value}}";
        let payload = json!({"value": null});
        let result = interpolate_command(cmd, &payload);
        assert_eq!(result.as_ref(), "echo ''");
    }

    // --- Additional coverage ---

    #[test]
    fn single_braces_are_not_placeholders() {
        let cmd = "echo {not_a_placeholder}";
        let payload = json!({"not_a_placeholder": "value"});
        let result = interpolate_command(cmd, &payload);
        assert!(matches!(result, Cow::Borrowed(_)));
        assert_eq!(result.as_ref(), "echo {not_a_placeholder}");
    }

    #[test]
    fn negative_number_field_interpolated() {
        let cmd = "offset={{delta}}";
        let payload = json!({"delta": -5});
        let result = interpolate_command(cmd, &payload);
        assert_eq!(result.as_ref(), "offset='-5'");
    }

    #[test]
    fn injection_via_double_quotes_is_escaped() {
        let cmd = "echo {{name}}";
        let payload = json!({"name": "a\"b"});
        let result = interpolate_command(cmd, &payload);
        // Double quotes inside single-quoted string are harmless
        assert_eq!(result.as_ref(), r#"echo 'a"b'"#);
    }

    #[test]
    fn injection_via_newline_is_escaped() {
        let cmd = "echo {{msg}}";
        let payload = json!({"msg": "line1\nrm -rf /"});
        let result = interpolate_command(cmd, &payload);
        assert_eq!(result.as_ref(), "echo 'line1\nrm -rf /'");
    }

    #[test]
    fn injection_via_pipe_is_escaped() {
        let cmd = "echo {{name}}";
        let payload = json!({"name": "foo | rm -rf /"});
        let result = interpolate_command(cmd, &payload);
        assert_eq!(result.as_ref(), "echo 'foo | rm -rf /'");
    }

    #[test]
    fn partial_dot_path_unresolved_left_as_is() {
        let cmd = "echo {{a.b.missing}}";
        let payload = json!({"a": {"b": {"c": "found"}}});
        let result = interpolate_command(cmd, &payload);
        assert_eq!(result.as_ref(), "echo {{a.b.missing}}");
    }

    #[test]
    fn dot_path_through_non_object_unresolved() {
        let cmd = "echo {{a.b}}";
        let payload = json!({"a": 42});
        let result = interpolate_command(cmd, &payload);
        assert_eq!(result.as_ref(), "echo {{a.b}}");
    }

    #[test]
    fn template_is_entirely_a_placeholder() {
        let cmd = "{{cmd}}";
        let payload = json!({"cmd": "ls -la"});
        let result = interpolate_command(cmd, &payload);
        assert_eq!(result.as_ref(), "'ls -la'");
    }

    #[test]
    fn adjacent_placeholders_both_interpolated() {
        let cmd = "{{a}}{{b}}";
        let payload = json!({"a": "hello", "b": "world"});
        let result = interpolate_command(cmd, &payload);
        assert_eq!(result.as_ref(), "'hello''world'");
    }

    #[test]
    fn underscore_and_digit_field_names() {
        let cmd = "echo {{_var1}} {{item_2}}";
        let payload = json!({"_var1": "alpha", "item_2": "beta"});
        let result = interpolate_command(cmd, &payload);
        assert_eq!(result.as_ref(), "echo 'alpha' 'beta'");
    }

    #[test]
    fn empty_payload_object_leaves_placeholders() {
        let cmd = "deploy {{app}} to {{env}}";
        let payload = json!({});
        let result = interpolate_command(cmd, &payload);
        assert_eq!(result.as_ref(), "deploy {{app}} to {{env}}");
    }

    #[test]
    fn float_zero_and_integer_zero() {
        let cmd = "{{a}} {{b}}";
        let payload = json!({"a": 0, "b": 0.0});
        let result = interpolate_command(cmd, &payload);
        assert_eq!(result.as_ref(), "'0' '0.0'");
    }

    #[test]
    fn nested_object_field_produces_json() {
        let cmd = "echo {{config.sub}}";
        let payload = json!({"config": {"sub": {"key": "val"}}});
        let result = interpolate_command(cmd, &payload);
        assert_eq!(result.as_ref(), r#"echo '{"key":"val"}'"#);
    }

    #[test]
    fn shell_escape_string_with_backslash() {
        assert_eq!(shell_escape(r"a\b"), r"'a\b'");
    }

    #[test]
    fn shell_escape_multiple_single_quotes() {
        assert_eq!(shell_escape("a'b'c"), "'a'\\''b'\\''c'");
    }
}
