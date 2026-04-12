use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Sentinel value used to replace secret content in displayed logs.
const MASK: &str = "***";

/// Raw deserialized masking configuration.
/// Patterns are strings that haven't been compiled yet.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct MaskingConfig {
    /// Env var names whose values should be masked in log output.
    #[serde(default)]
    pub env_vars: Vec<String>,
    /// Regex patterns whose matches should be masked in log output.
    /// Stored as raw strings for serialization; compiled forms are
    /// in `compiled_patterns`.
    #[serde(default)]
    pub patterns: Vec<String>,
    /// Compiled regex patterns. Populated by `compile()`, not by deserialization.
    #[serde(skip)]
    pub compiled_patterns: Vec<Regex>,
}

impl MaskingConfig {
    /// Compile all regex pattern strings into `Regex` objects.
    /// Returns an error listing all invalid patterns.
    pub fn compile(&mut self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        let mut compiled = Vec::with_capacity(self.patterns.len());

        for (i, pattern) in self.patterns.iter().enumerate() {
            match Regex::new(pattern) {
                Ok(re) => compiled.push(re),
                Err(e) => errors.push(format!(
                    "masking.patterns[{i}] '{}': {e}", pattern
                )),
            }
        }

        if errors.is_empty() {
            self.compiled_patterns = compiled;
            Ok(())
        } else {
            Err(errors)
        }
    }
}

/// Replace secret values in `text` with `***`.
///
/// Masking order:
/// 1. Env var values (literal string replacement)
/// 2. Regex patterns (in config order)
///
/// For env var resolution, `hook_env` is checked first, then the system
/// environment. This matches executor behavior where hook env overrides
/// system env.
pub fn mask_secrets(
    text: &str,
    config: &MaskingConfig,
    hook_env: &HashMap<String, String>,
) -> String {
    if config.env_vars.is_empty() && config.compiled_patterns.is_empty() {
        return text.to_owned();
    }

    let mut result = text.to_owned();

    // 1. Mask env var values (literal replacement)
    for var_name in &config.env_vars {
        let value = hook_env
            .get(var_name.as_str())
            .cloned()
            .or_else(|| std::env::var(var_name).ok());

        if let Some(val) = value
            && !val.is_empty() {
                result = result.replace(&val, MASK);
            }
    }

    // 2. Apply regex patterns
    for re in &config.compiled_patterns {
        result = re.replace_all(&result, MASK).into_owned();
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_config() -> MaskingConfig {
        MaskingConfig::default()
    }

    fn config_with_env(vars: &[&str]) -> MaskingConfig {
        MaskingConfig {
            env_vars: vars.iter().map(|s| (*s).into()).collect(),
            ..Default::default()
        }
    }

    fn config_with_patterns(patterns: &[&str]) -> MaskingConfig {
        let mut cfg = MaskingConfig {
            patterns: patterns.iter().map(|s| (*s).into()).collect(),
            ..Default::default()
        };
        cfg.compile().expect("test patterns should be valid");
        cfg
    }

    fn config_with_both(vars: &[&str], patterns: &[&str]) -> MaskingConfig {
        let mut cfg = MaskingConfig {
            env_vars: vars.iter().map(|s| (*s).into()).collect(),
            patterns: patterns.iter().map(|s| (*s).into()).collect(),
            ..Default::default()
        };
        cfg.compile().expect("test patterns should be valid");
        cfg
    }

    #[test]
    fn compile_reports_all_invalid_patterns() {
        let mut cfg = MaskingConfig {
            patterns: vec![
                "[valid".into(),
                r"ok_pattern".into(),
                "(unclosed".into(),
            ],
            ..Default::default()
        };
        let errors = cfg.compile().unwrap_err();
        assert_eq!(errors.len(), 2, "should report both invalid patterns");
        assert!(errors[0].contains("masking.patterns[0]"));
        assert!(errors[1].contains("masking.patterns[2]"));
    }

    #[test]
    fn empty_config_returns_text_unchanged() {
        let cfg = empty_config();
        let env = HashMap::new();
        let text = "some log output with secrets";
        let result = mask_secrets(text, &cfg, &env);
        assert_eq!(result, text);
    }

    #[test]
    fn env_var_masked_from_hook_env() {
        let cfg = config_with_env(&["SECRET_KEY"]);
        let env = HashMap::from([("SECRET_KEY".into(), "s3cr3t".into())]);
        let result = mask_secrets("connected with s3cr3t", &cfg, &env);
        assert_eq!(result, "connected with ***");
    }

    #[test]
    fn env_var_masked_from_system_env() {
        // Use a unique name to avoid collisions with parallel tests
        let var_name = "SENDWORD_TEST_MASK_SYSENV_7f3a";
        // SAFETY: tests are run in a single-threaded context per process,
        // and this var name is unique to this test.
        unsafe { std::env::set_var(var_name, "sys_secret") };

        let cfg = config_with_env(&[var_name]);
        let env = HashMap::new();
        let result = mask_secrets("value is sys_secret here", &cfg, &env);
        assert_eq!(result, "value is *** here");

        unsafe { std::env::remove_var(var_name) };
    }

    #[test]
    fn hook_env_takes_precedence_over_system_env() {
        let var_name = "SENDWORD_TEST_MASK_PREC_8b2c";
        unsafe { std::env::set_var(var_name, "system_val") };

        let cfg = config_with_env(&[var_name]);
        let env = HashMap::from([(var_name.into(), "hook_val".into())]);
        let text = "has hook_val and system_val";
        let result = mask_secrets(text, &cfg, &env);
        // Only hook_val should be masked (it's the resolved value)
        assert_eq!(result, "has *** and system_val");

        unsafe { std::env::remove_var(var_name) };
    }

    #[test]
    fn empty_env_var_value_is_not_masked() {
        let cfg = config_with_env(&["EMPTY_VAR"]);
        let env = HashMap::from([("EMPTY_VAR".into(), String::new())]);
        let text = "nothing should change";
        let result = mask_secrets(text, &cfg, &env);
        assert_eq!(result, text);
    }

    #[test]
    fn regex_pattern_masks_matched_content() {
        let cfg = config_with_patterns(&[r"Bearer [A-Za-z0-9._~+/=-]+"]);
        let env = HashMap::new();
        let result = mask_secrets("Authorization: Bearer abc123.xyz", &cfg, &env);
        assert_eq!(result, "Authorization: ***");
    }

    #[test]
    fn multiple_occurrences_of_same_secret_are_all_masked() {
        let cfg = config_with_env(&["TOKEN"]);
        let env = HashMap::from([("TOKEN".into(), "abc123".into())]);
        let text = "first abc123 then abc123 again abc123";
        let result = mask_secrets(text, &cfg, &env);
        assert_eq!(result, "first *** then *** again ***");
    }

    #[test]
    fn multiple_env_vars_and_patterns_all_applied() {
        let cfg = config_with_both(
            &["DB_PASS", "API_KEY"],
            &[r"ghp_[A-Za-z0-9]{8}"],
        );
        let env = HashMap::from([
            ("DB_PASS".into(), "dbpass123".into()),
            ("API_KEY".into(), "apikey456".into()),
        ]);
        let text = "db=dbpass123 api=apikey456 token=ghp_AbCdEfGh";
        let result = mask_secrets(text, &cfg, &env);
        assert_eq!(result, "db=*** api=*** token=***");
    }

    #[test]
    fn unresolvable_env_var_is_silently_skipped() {
        let cfg = config_with_env(&["NONEXISTENT_VAR_9x7z"]);
        let env = HashMap::new();
        let text = "nothing to mask here";
        let result = mask_secrets(text, &cfg, &env);
        assert_eq!(result, text);
    }

    #[test]
    fn env_var_masked_before_regex_prevents_double_replacement() {
        // If the env var value "secret123" also matches the regex pattern,
        // env var masking runs first (replacing "secret123" with "***"),
        // so the regex never sees the original value.
        let cfg = config_with_both(
            &["MY_TOKEN"],
            &[r"secret\d+"],
        );
        let env = HashMap::from([("MY_TOKEN".into(), "secret123".into())]);
        let text = "token is secret123 here";
        let result = mask_secrets(text, &cfg, &env);
        // After env var masking: "token is *** here"
        // Regex "secret\d+" does not match "***", so no double-replacement.
        assert_eq!(result, "token is *** here");
    }

    #[test]
    fn non_matching_patterns_leave_text_unchanged() {
        let cfg = config_with_both(
            &["NONEXISTENT_VAR_zz9q"],
            &[r"ghp_[A-Za-z0-9]{36}"],
        );
        let env = HashMap::new();
        let text = "nothing matches here at all";
        let result = mask_secrets(text, &cfg, &env);
        assert_eq!(result, text);
    }

    #[test]
    fn deleted_hook_env_empty_but_regex_still_masks() {
        // Simulates a hook that was deleted from config: hook_env is empty,
        // but regex patterns should still be applied.
        let cfg = config_with_both(
            &["DELETED_HOOK_SECRET"],
            &[r"Bearer [A-Za-z0-9._~+/=-]+"],
        );
        let env = HashMap::new(); // hook was deleted, no env map
        let text = "Authorization: Bearer abc123.xyz\nUsing key: actual_secret";
        let result = mask_secrets(text, &cfg, &env);
        assert!(result.contains("***"), "regex should still mask bearer token");
        assert!(result.contains("actual_secret"), "env var not resolvable, value not masked");
        assert_eq!(result, "Authorization: ***\nUsing key: actual_secret");
    }

    #[test]
    fn regex_with_capture_groups_replaces_entire_match() {
        let cfg = config_with_patterns(&[r"token=([A-Za-z0-9]+)"]);
        let env = HashMap::new();
        let result = mask_secrets("auth token=Abc123 done", &cfg, &env);
        assert_eq!(result, "auth *** done");
    }

    #[test]
    fn handler_wiring_pattern() {
        // Simulates the calling convention used in the execution detail handler
        let mut cfg = MaskingConfig {
            env_vars: vec!["APP_SECRET".into()],
            patterns: vec![r"Bearer [A-Za-z0-9._~+/=-]+".into()],
            ..Default::default()
        };
        cfg.compile().expect("valid patterns");

        let hook_env = HashMap::from([("APP_SECRET".into(), "production-key".into())]);

        let log_output = concat!(
            "Starting deploy...\n",
            "Using key: production-key\n",
            "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9\n",
            "Deploy complete.\n",
        );

        let result = mask_secrets(log_output, &cfg, &hook_env);

        assert!(!result.contains("production-key"), "env var value should be masked");
        assert!(!result.contains("Bearer eyJ"), "bearer token should be masked");
        assert!(result.contains("Starting deploy"), "non-secret content preserved");
        assert!(result.contains("Deploy complete"), "non-secret content preserved");
    }
}
