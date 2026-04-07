use regex::Regex;
use serde::Deserialize;
use std::collections::HashMap;

/// Sentinel value used to replace secret content in displayed logs.
const MASK: &str = "***";

/// Raw deserialized masking configuration.
/// Patterns are strings that haven't been compiled yet.
#[derive(Debug, Clone, Default, Deserialize)]
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

        if let Some(val) = value {
            if !val.is_empty() {
                result = result.replace(&val, MASK);
            }
        }
    }

    // 2. Apply regex patterns
    for re in &config.compiled_patterns {
        result = re.replace_all(&result, MASK).into_owned();
    }

    result
}
