use figment::{
    providers::{Env, Format, Json, Toml},
    Figment,
};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::time::Duration;

use crate::masking::MaskingConfig;
use crate::payload::PayloadSchema;

// --- Error type ---

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("config loading failed: {0}")]
    Figment(Box<figment::Error>),

    #[error("config validation failed:\n{0}")]
    Validation(String),
}

impl From<figment::Error> for ConfigError {
    fn from(err: figment::Error) -> Self {
        Self::Figment(Box::new(err))
    }
}

// --- Default value functions ---

fn default_bind() -> String {
    "127.0.0.1".into()
}

fn default_port() -> u16 {
    8080
}

fn default_db_path() -> String {
    "data/sendword.db".into()
}

fn default_logs_dir() -> String {
    "data/logs".into()
}

fn default_backoff() -> BackoffStrategy {
    BackoffStrategy::Exponential
}

fn default_initial_delay() -> Duration {
    Duration::from_secs(1)
}

fn default_max_delay() -> Duration {
    Duration::from_secs(60)
}

fn default_rate_limit() -> RateLimitConfig {
    RateLimitConfig { max_per_minute: 60 }
}

fn default_timeout() -> Duration {
    Duration::from_secs(30)
}

fn default_true() -> bool {
    true
}

fn default_session_lifetime() -> Duration {
    Duration::from_secs(24 * 60 * 60)
}

fn default_scripts_dir() -> String {
    "data/scripts".into()
}

// --- Config types ---

#[derive(Debug, Clone, Default, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub database: DatabaseConfig,
    #[serde(default)]
    pub logs: LogsConfig,
    #[serde(default)]
    pub auth: AuthConfig,
    #[serde(default)]
    pub scripts: ScriptsConfig,
    #[serde(default)]
    pub defaults: DefaultsConfig,
    #[serde(default)]
    pub masking: MaskingConfig,
    #[serde(default)]
    pub hooks: Vec<HookConfig>,
}

impl AppConfig {
    pub fn load() -> Result<Self, ConfigError> {
        Self::load_from("sendword.toml", "sendword.json")
    }

    pub fn load_from(toml_path: &str, json_path: &str) -> Result<Self, ConfigError> {
        let figment = Figment::new()
            .merge(Toml::file(toml_path))
            .merge(Json::file(json_path))
            .merge(Env::prefixed("SENDWORD_").split("__"));

        let mut config: AppConfig = figment.extract()?;
        if let Err(errors) = config.masking.compile() {
            return Err(ConfigError::Validation(errors.join("\n")));
        }
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<(), ConfigError> {
        let mut errors = Vec::new();

        if self.server.port == 0 {
            errors.push("server.port must be non-zero".into());
        }

        if self.defaults.rate_limit.max_per_minute == 0 {
            errors.push("defaults.rate_limit.max_per_minute must be greater than 0".into());
        }

        if self.auth.session_lifetime == Duration::ZERO {
            errors.push("auth.session_lifetime must be greater than 0".into());
        }

        if self.scripts.dir.is_empty() {
            errors.push("scripts.dir must be non-empty".into());
        }

        if self.defaults.timeout == Duration::ZERO {
            errors.push("defaults.timeout must be greater than 0".into());
        }

        if self.defaults.retries.initial_delay > self.defaults.retries.max_delay {
            errors.push(
                "defaults.retries.initial_delay must not exceed defaults.retries.max_delay".into(),
            );
        }

        for (i, name) in self.masking.env_vars.iter().enumerate() {
            if name.is_empty() {
                errors.push(format!("masking.env_vars[{i}] must be non-empty"));
            }
        }

        let mut seen_slugs = HashSet::with_capacity(self.hooks.len());

        for (i, hook) in self.hooks.iter().enumerate() {
            let prefix = format!("hooks[{i}]");

            if hook.name.is_empty() {
                errors.push(format!("{prefix}.name must be non-empty"));
            }

            if !is_valid_slug(&hook.slug) {
                errors.push(format!(
                    "{prefix}.slug '{}' is invalid (must be 1-64 lowercase alphanumeric \
                     chars or hyphens, no leading/trailing/consecutive hyphens)",
                    hook.slug,
                ));
            }

            if !seen_slugs.insert(&hook.slug) {
                errors.push(format!("{prefix}.slug '{}' is a duplicate", hook.slug));
            }

            match &hook.executor {
                ExecutorConfig::Shell { command } if command.is_empty() => {
                    errors.push(format!("{prefix}.executor.command must be non-empty"));
                }
                _ => {}
            }

            if let Some(retries) = &hook.retries
                && retries.initial_delay > retries.max_delay
            {
                errors.push(format!(
                    "{prefix}.retries.initial_delay must not exceed {prefix}.retries.max_delay",
                ));
            }

            if let Some(rl) = &hook.rate_limit
                && rl.max_per_minute == 0
            {
                errors.push(format!(
                    "{prefix}.rate_limit.max_per_minute must be greater than 0",
                ));
            }

            if let Some(auth) = &hook.auth {
                match auth {
                    HookAuthConfig::None => {}
                    HookAuthConfig::Bearer { token } => {
                        if token.is_empty() {
                            errors.push(format!(
                                "{prefix}.auth.token must be non-empty"
                            ));
                        }
                    }
                    HookAuthConfig::Hmac { header, algorithm: _, secret } => {
                        if header.is_empty() {
                            errors.push(format!(
                                "{prefix}.auth.header must be non-empty"
                            ));
                        }
                        if secret.is_empty() {
                            errors.push(format!(
                                "{prefix}.auth.secret must be non-empty"
                            ));
                        }
                    }
                }
            }

            if let Some(rules) = &hook.trigger_rules {
                if let Some(filters) = &rules.payload_filters {
                    for (j, filter) in filters.iter().enumerate() {
                        if filter.operator == FilterOperator::Regex {
                            match &filter.value {
                                Some(pattern) => {
                                    if regex::Regex::new(pattern).is_err() {
                                        errors.push(format!(
                                            "{prefix}.trigger_rules.payload_filters[{j}].value \
                                             is not a valid regex"
                                        ));
                                    }
                                }
                                None => {
                                    errors.push(format!(
                                        "{prefix}.trigger_rules.payload_filters[{j}].value \
                                         is required for regex operator"
                                    ));
                                }
                            }
                        }
                    }
                }

                if let Some(windows) = &rules.time_windows {
                    for (j, window) in windows.iter().enumerate() {
                        let prefix_w =
                            format!("{prefix}.trigger_rules.time_windows[{j}]");
                        if chrono::NaiveTime::parse_from_str(&window.start_time, "%H:%M").is_err()
                        {
                            errors.push(format!("{prefix_w}.start_time must be HH:MM format"));
                        }
                        if chrono::NaiveTime::parse_from_str(&window.end_time, "%H:%M").is_err() {
                            errors.push(format!("{prefix_w}.end_time must be HH:MM format"));
                        }
                        if let (Ok(start), Ok(end)) = (
                            chrono::NaiveTime::parse_from_str(&window.start_time, "%H:%M"),
                            chrono::NaiveTime::parse_from_str(&window.end_time, "%H:%M"),
                        ) {
                            if start >= end {
                                errors.push(format!(
                                    "{prefix_w}.start_time must be before end_time"
                                ));
                            }
                        }
                        const VALID_DAYS: &[&str] =
                            &["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
                        for day in &window.days {
                            if !VALID_DAYS.iter().any(|d| d.eq_ignore_ascii_case(day)) {
                                errors.push(format!(
                                    "{prefix_w}.days contains invalid day '{day}' \
                                     (expected Mon-Sun)"
                                ));
                            }
                        }
                    }
                }

                if let Some(rl) = &rules.rate_limit {
                    if rl.max_requests == 0 {
                        errors.push(format!(
                            "{prefix}.trigger_rules.rate_limit.max_requests must be greater than 0"
                        ));
                    }
                    if rl.window.is_zero() {
                        errors.push(format!(
                            "{prefix}.trigger_rules.rate_limit.window must be greater than 0"
                        ));
                    }
                }
            }

            if let Some(concurrency) = &hook.concurrency {
                if concurrency.mode == ConcurrencyMode::Queue && concurrency.queue_depth == 0 {
                    errors.push(format!(
                        "{prefix}.concurrency.queue_depth must be greater than 0 in queue mode"
                    ));
                }
            }

            if let Some(approval) = &hook.approval {
                if let Some(timeout) = approval.timeout {
                    if timeout.is_zero() {
                        errors.push(format!(
                            "{prefix}.approval.timeout must be greater than 0 if set"
                        ));
                    }
                }
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(ConfigError::Validation(errors.join("\n")))
        }
    }
}

fn is_valid_slug(s: &str) -> bool {
    let len = s.len();
    if len == 0 || len > 64 {
        return false;
    }

    let bytes = s.as_bytes();
    if bytes[0] == b'-' || bytes[len - 1] == b'-' {
        return false;
    }

    let mut prev_hyphen = false;
    for &b in bytes {
        match b {
            b'a'..=b'z' | b'0'..=b'9' => prev_hyphen = false,
            b'-' => {
                if prev_hyphen {
                    return false;
                }
                prev_hyphen = true;
            }
            _ => return false,
        }
    }

    true
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_bind")]
    pub bind: String,
    #[serde(default = "default_port")]
    pub port: u16,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind: default_bind(),
            port: default_port(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct DatabaseConfig {
    #[serde(default = "default_db_path")]
    pub path: String,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            path: default_db_path(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct LogsConfig {
    #[serde(default = "default_logs_dir")]
    pub dir: String,
}

impl Default for LogsConfig {
    fn default() -> Self {
        Self {
            dir: default_logs_dir(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct AuthConfig {
    #[serde(default = "default_session_lifetime", with = "humantime_serde")]
    pub session_lifetime: Duration,
    #[serde(default)]
    pub secure_cookie: bool,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            session_lifetime: default_session_lifetime(),
            secure_cookie: false,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ScriptsConfig {
    #[serde(default = "default_scripts_dir")]
    pub dir: String,
}

impl Default for ScriptsConfig {
    fn default() -> Self {
        Self {
            dir: default_scripts_dir(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct RateLimitConfig {
    pub max_per_minute: u32,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        default_rate_limit()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackoffStrategy {
    None,
    Linear,
    Exponential,
}

impl Default for BackoffStrategy {
    fn default() -> Self {
        default_backoff()
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct RetryConfig {
    #[serde(default)]
    pub count: u32,
    #[serde(default = "default_backoff")]
    pub backoff: BackoffStrategy,
    #[serde(default = "default_initial_delay", with = "humantime_serde")]
    pub initial_delay: Duration,
    #[serde(default = "default_max_delay", with = "humantime_serde")]
    pub max_delay: Duration,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            count: 0,
            backoff: default_backoff(),
            initial_delay: default_initial_delay(),
            max_delay: default_max_delay(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct DefaultsConfig {
    #[serde(default = "default_rate_limit")]
    pub rate_limit: RateLimitConfig,
    #[serde(default = "default_timeout", with = "humantime_serde")]
    pub timeout: Duration,
    #[serde(default)]
    pub retries: RetryConfig,
}

impl Default for DefaultsConfig {
    fn default() -> Self {
        Self {
            rate_limit: default_rate_limit(),
            timeout: default_timeout(),
            retries: RetryConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExecutorConfig {
    Shell { command: String },
    Script { path: String },
    Http {
        method: HttpMethod,
        url: String,
        #[serde(default)]
        headers: HashMap<String, String>,
        #[serde(default)]
        body: Option<String>,
        #[serde(default = "default_true")]
        follow_redirects: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HmacAlgorithm {
    Sha256,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum HookAuthConfig {
    None,
    Bearer {
        token: String,
    },
    Hmac {
        header: String,
        algorithm: HmacAlgorithm,
        secret: String,
    },
}

// --- Trigger rules ---

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilterOperator {
    Equals,
    NotEquals,
    Contains,
    Regex,
    Exists,
    Gt,
    Lt,
    Gte,
    Lte,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PayloadFilter {
    pub field: String,
    pub operator: FilterOperator,
    #[serde(default)]
    pub value: Option<String>,
}

/// A time window during which a hook is allowed to execute.
/// `days` is a list of day names (e.g. ["Mon", "Tue", "Wed"]).
/// Times are UTC strings in "HH:MM" format.
#[derive(Debug, Clone, Deserialize)]
pub struct TimeWindow {
    pub days: Vec<String>,
    pub start_time: String,
    pub end_time: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TriggerRateLimit {
    pub max_requests: u64,
    #[serde(with = "humantime_serde")]
    pub window: Duration,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TriggerRules {
    #[serde(default)]
    pub payload_filters: Option<Vec<PayloadFilter>>,
    #[serde(default)]
    pub time_windows: Option<Vec<TimeWindow>>,
    #[serde(default, with = "humantime_serde::option")]
    pub cooldown: Option<Duration>,
    #[serde(default)]
    pub rate_limit: Option<TriggerRateLimit>,
}

// --- Execution barriers ---

fn default_queue_depth() -> u32 {
    10
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConcurrencyMode {
    Mutex,
    Queue,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ConcurrencyConfig {
    pub mode: ConcurrencyMode,
    #[serde(default = "default_queue_depth")]
    pub queue_depth: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ApprovalConfig {
    pub required: bool,
    #[serde(default, with = "humantime_serde::option")]
    pub timeout: Option<Duration>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HookConfig {
    pub name: String,
    pub slug: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub auth: Option<HookAuthConfig>,
    pub executor: ExecutorConfig,
    #[serde(default)]
    pub env: HashMap<String, String>,
    pub cwd: Option<String>,
    #[serde(default, with = "humantime_serde::option")]
    pub timeout: Option<Duration>,
    pub retries: Option<RetryConfig>,
    pub rate_limit: Option<RateLimitConfig>,
    pub payload: Option<PayloadSchema>,
    #[serde(default)]
    pub trigger_rules: Option<TriggerRules>,
    #[serde(default)]
    pub concurrency: Option<ConcurrencyConfig>,
    #[serde(default)]
    pub approval: Option<ApprovalConfig>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use figment::providers::Data;

    #[test]
    fn default_config_loads_when_no_files_exist() {
        figment::Jail::expect_with(|_jail| {
            let config: AppConfig = Figment::new()
                .merge(Toml::file("nonexistent.toml"))
                .merge(Json::file("nonexistent.json"))
                .extract()?;

            assert!(config.hooks.is_empty());
            assert_eq!(config.server.bind, "127.0.0.1");
            assert_eq!(config.server.port, 8080);
            assert_eq!(config.database.path, "data/sendword.db");
            assert_eq!(config.logs.dir, "data/logs");
            assert_eq!(config.defaults.rate_limit.max_per_minute, 60);
            assert_eq!(config.defaults.timeout, Duration::from_secs(30));
            assert_eq!(config.defaults.retries.count, 0);
            assert_eq!(config.defaults.retries.backoff, BackoffStrategy::Exponential);
            assert_eq!(config.defaults.retries.initial_delay, Duration::from_secs(1));
            assert_eq!(config.defaults.retries.max_delay, Duration::from_secs(60));
            assert_eq!(config.auth.session_lifetime, Duration::from_secs(24 * 60 * 60));
            assert!(!config.auth.secure_cookie);
            assert_eq!(config.scripts.dir, "data/scripts");
            Ok(())
        });
    }

    #[test]
    fn minimal_toml_produces_correct_defaults() {
        figment::Jail::expect_with(|_jail| {
            let config: AppConfig = Figment::new()
                .merge(Data::<Toml>::string("[server]\nport = 9090"))
                .extract()?;

            assert_eq!(config.server.port, 9090);
            assert_eq!(config.server.bind, "127.0.0.1");
            assert_eq!(config.database.path, "data/sendword.db");
            assert!(config.hooks.is_empty());
            Ok(())
        });
    }

    #[test]
    fn complete_toml_deserializes_correctly() {
        figment::Jail::expect_with(|_jail| {
            let toml = r#"
                [server]
                bind = "0.0.0.0"
                port = 3000

                [database]
                path = "/var/lib/sendword.db"

                [logs]
                dir = "/var/log/sendword"

                [defaults]
                timeout = "60s"

                [defaults.rate_limit]
                max_per_minute = 120

                [defaults.retries]
                count = 3
                backoff = "linear"
                initial_delay = "2s"
                max_delay = "30s"

                [[hooks]]
                name = "Deploy"
                slug = "deploy"
                description = "Deploy the app"
                enabled = false
                cwd = "/opt/app"
                timeout = "120s"

                [hooks.executor]
                type = "shell"
                command = "make deploy"

                [hooks.env]
                APP_ENV = "production"

                [hooks.retries]
                count = 2
                backoff = "exponential"
                initial_delay = "5s"
                max_delay = "60s"

                [hooks.rate_limit]
                max_per_minute = 10
            "#;

            let config: AppConfig = Figment::new()
                .merge(Data::<Toml>::string(toml))
                .extract()?;

            assert_eq!(config.server.bind, "0.0.0.0");
            assert_eq!(config.server.port, 3000);
            assert_eq!(config.database.path, "/var/lib/sendword.db");
            assert_eq!(config.logs.dir, "/var/log/sendword");
            assert_eq!(config.defaults.timeout, Duration::from_secs(60));
            assert_eq!(config.defaults.rate_limit.max_per_minute, 120);
            assert_eq!(config.defaults.retries.count, 3);
            assert_eq!(config.defaults.retries.backoff, BackoffStrategy::Linear);

            assert_eq!(config.hooks.len(), 1);
            let hook = &config.hooks[0];
            assert_eq!(hook.name, "Deploy");
            assert_eq!(hook.slug, "deploy");
            assert_eq!(hook.description, "Deploy the app");
            assert!(!hook.enabled);
            assert_eq!(hook.cwd.as_deref(), Some("/opt/app"));
            assert_eq!(hook.timeout, Some(Duration::from_secs(120)));

            let ExecutorConfig::Shell { command } = &hook.executor else {
                panic!("expected Shell executor");
            };
            assert_eq!(command, "make deploy");

            assert_eq!(hook.env.get("APP_ENV").map(String::as_str), Some("production"));

            let retries = hook.retries.as_ref().expect("retries should be Some");
            assert_eq!(retries.count, 2);
            assert_eq!(retries.backoff, BackoffStrategy::Exponential);

            let rl = hook.rate_limit.as_ref().expect("rate_limit should be Some");
            assert_eq!(rl.max_per_minute, 10);

            Ok(())
        });
    }

    #[test]
    fn auth_and_scripts_config_from_toml() {
        figment::Jail::expect_with(|_jail| {
            let toml = r#"
                [auth]
                session_lifetime = "7d"
                secure_cookie = true

                [scripts]
                dir = "/opt/sendword/scripts"
            "#;

            let config: AppConfig = Figment::new()
                .merge(Data::<Toml>::string(toml))
                .extract()?;

            assert_eq!(config.auth.session_lifetime, Duration::from_secs(7 * 24 * 60 * 60));
            assert!(config.auth.secure_cookie);
            assert_eq!(config.scripts.dir, "/opt/sendword/scripts");
            Ok(())
        });
    }

    #[test]
    fn hook_with_optional_fields_omitted_gets_none() {
        figment::Jail::expect_with(|_jail| {
            let toml = r#"
                [[hooks]]
                name = "Simple"
                slug = "simple"

                [hooks.executor]
                type = "shell"
                command = "echo hello"
            "#;

            let config: AppConfig = Figment::new()
                .merge(Data::<Toml>::string(toml))
                .extract()?;

            let hook = &config.hooks[0];
            assert!(hook.cwd.is_none());
            assert!(hook.timeout.is_none());
            assert!(hook.retries.is_none());
            assert!(hook.rate_limit.is_none());
            assert!(hook.enabled);
            assert!(hook.description.is_empty());
            Ok(())
        });
    }

    #[test]
    fn json_overrides_toml_values() {
        figment::Jail::expect_with(|jail| {
            jail.create_file("test.toml", r#"
                [server]
                bind = "127.0.0.1"
                port = 8080
            "#)?;

            jail.create_file("test.json", r#"
                {
                    "server": {
                        "port": 9090
                    }
                }
            "#)?;

            let config = AppConfig::load_from("test.toml", "test.json")
                .map_err(|e| e.to_string())?;
            assert_eq!(config.server.port, 9090);
            assert_eq!(config.server.bind, "127.0.0.1");
            Ok(())
        });
    }

    #[test]
    fn env_var_overrides_work() {
        figment::Jail::expect_with(|jail| {
            jail.create_file("test.toml", r#"
                [server]
                port = 8080
            "#)?;

            jail.set_env("SENDWORD_SERVER__PORT", "9999");

            let config = AppConfig::load_from("test.toml", "nonexistent.json")
                .map_err(|e| e.to_string())?;
            assert_eq!(config.server.port, 9999);
            Ok(())
        });
    }

    #[test]
    fn backoff_strategy_deserialization() {
        figment::Jail::expect_with(|_jail| {
            let toml_none = r#"
                [defaults.retries]
                backoff = "none"
            "#;
            let config: AppConfig = Figment::new()
                .merge(Data::<Toml>::string(toml_none))
                .extract()?;
            assert_eq!(config.defaults.retries.backoff, BackoffStrategy::None);

            let toml_linear = r#"
                [defaults.retries]
                backoff = "linear"
            "#;
            let config: AppConfig = Figment::new()
                .merge(Data::<Toml>::string(toml_linear))
                .extract()?;
            assert_eq!(config.defaults.retries.backoff, BackoffStrategy::Linear);

            let toml_exp = r#"
                [defaults.retries]
                backoff = "exponential"
            "#;
            let config: AppConfig = Figment::new()
                .merge(Data::<Toml>::string(toml_exp))
                .extract()?;
            assert_eq!(config.defaults.retries.backoff, BackoffStrategy::Exponential);

            Ok(())
        });
    }

    #[test]
    fn duration_fields_from_human_readable_strings() {
        figment::Jail::expect_with(|_jail| {
            let toml = r#"
                [defaults]
                timeout = "5m"

                [defaults.retries]
                initial_delay = "500ms"
                max_delay = "2h"
            "#;

            let config: AppConfig = Figment::new()
                .merge(Data::<Toml>::string(toml))
                .extract()?;

            assert_eq!(config.defaults.timeout, Duration::from_secs(300));
            assert_eq!(config.defaults.retries.initial_delay, Duration::from_millis(500));
            assert_eq!(config.defaults.retries.max_delay, Duration::from_secs(7200));
            Ok(())
        });
    }

    // --- Validation tests ---

    fn make_hook(name: &str, slug: &str, command: &str) -> HookConfig {
        HookConfig {
            name: name.into(),
            slug: slug.into(),
            description: String::new(),
            enabled: true,
            auth: None,
            executor: ExecutorConfig::Shell {
                command: command.into(),
            },
            env: HashMap::new(),
            cwd: None,
            timeout: None,
            retries: None,
            rate_limit: None,
            payload: None,
            trigger_rules: None,
            concurrency: None,
            approval: None,
        }
    }

    fn valid_config_with_hooks(hooks: Vec<HookConfig>) -> AppConfig {
        AppConfig {
            hooks,
            ..AppConfig::default()
        }
    }

    #[test]
    fn is_valid_slug_accepts_valid() {
        assert!(is_valid_slug("deploy"));
        assert!(is_valid_slug("my-hook"));
        assert!(is_valid_slug("a"));
        assert!(is_valid_slug("a1"));
        assert!(is_valid_slug("deploy-app-v2"));
    }

    #[test]
    fn is_valid_slug_rejects_invalid() {
        assert!(!is_valid_slug(""));
        assert!(!is_valid_slug("-deploy"));
        assert!(!is_valid_slug("deploy-"));
        assert!(!is_valid_slug("DEPLOY"));
        assert!(!is_valid_slug("deploy app"));
        assert!(!is_valid_slug("deploy--app"));
        assert!(!is_valid_slug(&"a".repeat(65)));
    }

    #[test]
    fn validation_catches_duplicate_slugs() {
        let config = valid_config_with_hooks(vec![
            make_hook("Hook A", "deploy", "echo a"),
            make_hook("Hook B", "deploy", "echo b"),
        ]);
        let err = config.validate().unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("deploy"), "error should name the duplicate slug: {msg}");
        assert!(msg.contains("duplicate"), "error should mention duplicate: {msg}");
    }

    #[test]
    fn validation_catches_empty_hook_name() {
        let config = valid_config_with_hooks(vec![make_hook("", "valid-slug", "echo ok")]);
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("name must be non-empty"));
    }

    #[test]
    fn validation_catches_empty_shell_command() {
        let config = valid_config_with_hooks(vec![make_hook("Deploy", "deploy", "")]);
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("command must be non-empty"));
    }

    #[test]
    fn validation_catches_invalid_slug_format() {
        let config = valid_config_with_hooks(vec![make_hook("Deploy", "INVALID", "echo ok")]);
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("slug 'INVALID' is invalid"));
    }

    #[test]
    fn validation_catches_zero_port() {
        let mut config = AppConfig::default();
        config.server.port = 0;
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("port must be non-zero"));
    }

    #[test]
    fn validation_catches_zero_rate_limit() {
        let mut config = AppConfig::default();
        config.defaults.rate_limit.max_per_minute = 0;
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("max_per_minute must be greater than 0"));
    }

    #[test]
    fn validation_catches_zero_timeout() {
        let mut config = AppConfig::default();
        config.defaults.timeout = Duration::ZERO;
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("timeout must be greater than 0"));
    }

    #[test]
    fn validation_catches_initial_delay_exceeds_max_delay() {
        let mut config = AppConfig::default();
        config.defaults.retries.initial_delay = Duration::from_secs(120);
        config.defaults.retries.max_delay = Duration::from_secs(60);
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("initial_delay must not exceed"));
    }

    #[test]
    fn validation_catches_hook_retry_initial_exceeds_max() {
        let mut hook = make_hook("Deploy", "deploy", "echo ok");
        hook.retries = Some(RetryConfig {
            count: 3,
            backoff: BackoffStrategy::Linear,
            initial_delay: Duration::from_secs(60),
            max_delay: Duration::from_secs(10),
        });
        let config = valid_config_with_hooks(vec![hook]);
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("hooks[0].retries.initial_delay must not exceed"));
    }

    #[test]
    fn validation_catches_hook_zero_rate_limit() {
        let mut hook = make_hook("Deploy", "deploy", "echo ok");
        hook.rate_limit = Some(RateLimitConfig { max_per_minute: 0 });
        let config = valid_config_with_hooks(vec![hook]);
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("hooks[0].rate_limit.max_per_minute must be greater than 0"));
    }

    #[test]
    fn validation_reports_multiple_errors_at_once() {
        let mut config = AppConfig::default();
        config.server.port = 0;
        config.defaults.rate_limit.max_per_minute = 0;
        config.defaults.timeout = Duration::ZERO;
        let err = config.validate().unwrap_err();
        let msg = err.to_string();
        // Should contain at least 3 distinct error lines
        let error_lines: Vec<&str> = msg.lines().filter(|l| !l.is_empty()).collect();
        assert!(
            error_lines.len() >= 3,
            "expected at least 3 errors, got {}:\n{msg}",
            error_lines.len()
        );
    }

    #[test]
    fn validation_catches_zero_session_lifetime() {
        let mut config = AppConfig::default();
        config.auth.session_lifetime = Duration::ZERO;
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("session_lifetime must be greater than 0"));
    }

    #[test]
    fn validation_catches_empty_scripts_dir() {
        let mut config = AppConfig::default();
        config.scripts.dir = String::new();
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("scripts.dir must be non-empty"));
    }

    #[test]
    fn validation_passes_for_valid_config_with_multiple_hooks() {
        let config = valid_config_with_hooks(vec![
            make_hook("Deploy App", "deploy-app", "make deploy"),
            make_hook("Run Tests", "run-tests", "make test"),
            make_hook("Backup DB", "backup-db", "pg_dump > backup.sql"),
        ]);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn load_from_rejects_config_that_fails_validation() {
        figment::Jail::expect_with(|jail| {
            // TOML is valid syntax and deserializes fine, but port=0 fails validation
            jail.create_file(
                "test.toml",
                r#"
                [server]
                port = 0
            "#,
            )?;

            let result = AppConfig::load_from("test.toml", "nonexistent.json");
            assert!(result.is_err(), "load_from should reject config with port=0");
            let msg = result.unwrap_err().to_string();
            assert!(
                msg.contains("port must be non-zero"),
                "error should mention the validation failure: {msg}"
            );
            Ok(())
        });
    }

    #[test]
    fn is_valid_slug_accepts_max_length() {
        // Slugs up to 64 chars are valid; this is the upper boundary
        let slug_64 = "a".repeat(64);
        assert!(is_valid_slug(&slug_64));
    }

    // --- Auth config tests ---

    #[test]
    fn hook_with_bearer_auth_deserializes() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("HOOK_TOKEN", "secret123");
            let toml = r#"
                [[hooks]]
                name = "Authed"
                slug = "authed"
                [hooks.executor]
                type = "shell"
                command = "echo ok"
                [hooks.auth]
                mode = "bearer"
                token = "${HOOK_TOKEN}"
            "#;
            let config: AppConfig = Figment::new()
                .merge(Data::<Toml>::string(toml))
                .extract()?;
            let hook = &config.hooks[0];
            let auth = hook.auth.as_ref().expect("auth should be present");
            match auth {
                HookAuthConfig::Bearer { token } => {
                    assert_eq!(token, "${HOOK_TOKEN}");
                }
                _ => panic!("expected bearer auth"),
            }
            Ok(())
        });
    }

    #[test]
    fn hook_with_hmac_auth_deserializes() {
        figment::Jail::expect_with(|_jail| {
            let toml = r#"
                [[hooks]]
                name = "HMAC"
                slug = "hmac-hook"
                [hooks.executor]
                type = "shell"
                command = "echo ok"
                [hooks.auth]
                mode = "hmac"
                header = "X-Hub-Signature-256"
                algorithm = "sha256"
                secret = "${WEBHOOK_SECRET}"
            "#;
            let config: AppConfig = Figment::new()
                .merge(Data::<Toml>::string(toml))
                .extract()?;
            let hook = &config.hooks[0];
            let auth = hook.auth.as_ref().expect("auth should be present");
            match auth {
                HookAuthConfig::Hmac { header, algorithm, secret } => {
                    assert_eq!(header, "X-Hub-Signature-256");
                    assert_eq!(*algorithm, HmacAlgorithm::Sha256);
                    assert_eq!(secret, "${WEBHOOK_SECRET}");
                }
                _ => panic!("expected hmac auth"),
            }
            Ok(())
        });
    }

    #[test]
    fn hook_with_no_auth_defaults_to_none() {
        figment::Jail::expect_with(|_jail| {
            let toml = r#"
                [[hooks]]
                name = "Public"
                slug = "public"
                [hooks.executor]
                type = "shell"
                command = "echo ok"
            "#;
            let config: AppConfig = Figment::new()
                .merge(Data::<Toml>::string(toml))
                .extract()?;
            assert!(config.hooks[0].auth.is_none());
            Ok(())
        });
    }

    #[test]
    fn hook_with_explicit_none_auth_deserializes() {
        figment::Jail::expect_with(|_jail| {
            let toml = r#"
                [[hooks]]
                name = "Explicit None"
                slug = "explicit-none"
                [hooks.executor]
                type = "shell"
                command = "echo ok"
                [hooks.auth]
                mode = "none"
            "#;
            let config: AppConfig = Figment::new()
                .merge(Data::<Toml>::string(toml))
                .extract()?;
            let hook = &config.hooks[0];
            let auth = hook.auth.as_ref().expect("auth should be present");
            assert!(matches!(auth, HookAuthConfig::None));
            Ok(())
        });
    }

    #[test]
    fn bearer_auth_with_empty_token_fails_validation() {
        figment::Jail::expect_with(|_jail| {
            let toml = r#"
                [[hooks]]
                name = "Bad"
                slug = "bad"
                [hooks.executor]
                type = "shell"
                command = "echo ok"
                [hooks.auth]
                mode = "bearer"
                token = ""
            "#;
            let config: AppConfig = Figment::new()
                .merge(Data::<Toml>::string(toml))
                .extract()?;
            let result = config.validate();
            assert!(result.is_err());
            let err = result.unwrap_err().to_string();
            assert!(err.contains("token"), "error should mention token: {err}");
            Ok(())
        });
    }

    #[test]
    fn hmac_auth_with_empty_header_fails_validation() {
        figment::Jail::expect_with(|_jail| {
            let toml = r#"
                [[hooks]]
                name = "Bad"
                slug = "bad"
                [hooks.executor]
                type = "shell"
                command = "echo ok"
                [hooks.auth]
                mode = "hmac"
                header = ""
                algorithm = "sha256"
                secret = "abc"
            "#;
            let config: AppConfig = Figment::new()
                .merge(Data::<Toml>::string(toml))
                .extract()?;
            let result = config.validate();
            assert!(result.is_err());
            let err = result.unwrap_err().to_string();
            assert!(err.contains("header"), "error should mention header: {err}");
            Ok(())
        });
    }

    #[test]
    fn hmac_auth_with_empty_secret_fails_validation() {
        figment::Jail::expect_with(|_jail| {
            let toml = r#"
                [[hooks]]
                name = "Bad"
                slug = "bad"
                [hooks.executor]
                type = "shell"
                command = "echo ok"
                [hooks.auth]
                mode = "hmac"
                header = "X-Sig"
                algorithm = "sha256"
                secret = ""
            "#;
            let config: AppConfig = Figment::new()
                .merge(Data::<Toml>::string(toml))
                .extract()?;
            let result = config.validate();
            assert!(result.is_err());
            let err = result.unwrap_err().to_string();
            assert!(err.contains("secret"), "error should mention secret: {err}");
            Ok(())
        });
    }

    // --- Masking config tests ---

    #[test]
    fn masking_config_deserializes_and_compiles() {
        figment::Jail::expect_with(|jail| {
            jail.create_file(
                "test.toml",
                r#"
                [masking]
                env_vars = ["SECRET_KEY"]
                patterns = ["Bearer [A-Za-z0-9]+"]
            "#,
            )?;

            let config = AppConfig::load_from("test.toml", "nonexistent.json")
                .map_err(|e| e.to_string())?;
            assert_eq!(config.masking.env_vars, vec!["SECRET_KEY"]);
            assert_eq!(config.masking.patterns.len(), 1);
            assert_eq!(config.masking.compiled_patterns.len(), 1);
            Ok(())
        });
    }

    #[test]
    fn masking_config_invalid_regex_fails_load() {
        figment::Jail::expect_with(|jail| {
            jail.create_file(
                "test.toml",
                r#"
                [masking]
                patterns = ["[invalid("]
            "#,
            )?;

            let result = AppConfig::load_from("test.toml", "nonexistent.json");
            assert!(result.is_err());
            let msg = result.unwrap_err().to_string();
            assert!(
                msg.contains("masking.patterns[0]"),
                "error should reference the pattern index: {msg}"
            );
            Ok(())
        });
    }

    #[test]
    fn masking_config_defaults_to_empty() {
        figment::Jail::expect_with(|_jail| {
            let config: AppConfig = Figment::new()
                .merge(Toml::file("nonexistent.toml"))
                .merge(Json::file("nonexistent.json"))
                .extract()?;

            assert!(config.masking.env_vars.is_empty());
            assert!(config.masking.patterns.is_empty());
            assert!(config.masking.compiled_patterns.is_empty());
            Ok(())
        });
    }

    #[test]
    fn masking_config_empty_section_defaults_to_empty() {
        figment::Jail::expect_with(|jail| {
            jail.create_file(
                "test.toml",
                r#"
                [masking]
            "#,
            )?;

            let config = AppConfig::load_from("test.toml", "nonexistent.json")
                .map_err(|e| e.to_string())?;
            assert!(config.masking.env_vars.is_empty());
            assert!(config.masking.patterns.is_empty());
            Ok(())
        });
    }

    #[test]
    fn masking_config_empty_env_var_name_fails_validation() {
        let mut config = AppConfig::default();
        config.masking.env_vars = vec!["".into()];
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("masking.env_vars[0] must be non-empty"));
    }

    #[test]
    fn hook_with_payload_schema_loads() {
        figment::Jail::expect_with(|jail| {
            jail.create_file(
                "sendword.toml",
                r#"
                [server]
                port = 8080

                [[hooks]]
                name = "Deploy"
                slug = "deploy"
                [hooks.executor]
                type = "shell"
                command = "echo deploy"
                [[hooks.payload.fields]]
                name = "action"
                type = "string"
                required = true
                [[hooks.payload.fields]]
                name = "tag"
                type = "string"
                required = false
                "#,
            )?;
            let config = AppConfig::load_from("sendword.toml", "nonexistent.json")
                .expect("should load");
            let hook = &config.hooks[0];
            let schema = hook.payload.as_ref().expect("payload schema should be present");
            assert_eq!(schema.fields.len(), 2);
            assert_eq!(schema.fields[0].name, "action");
            assert!(schema.fields[0].required);
            assert_eq!(schema.fields[1].name, "tag");
            assert!(!schema.fields[1].required);
            Ok(())
        });
    }

    #[test]
    fn hook_without_payload_schema_loads_as_none() {
        figment::Jail::expect_with(|jail| {
            jail.create_file(
                "sendword.toml",
                r#"
                [server]
                port = 8080

                [[hooks]]
                name = "Simple"
                slug = "simple"
                [hooks.executor]
                type = "shell"
                command = "echo hi"
                "#,
            )?;
            let config = AppConfig::load_from("sendword.toml", "nonexistent.json")
                .expect("should load");
            assert!(config.hooks[0].payload.is_none());
            Ok(())
        });
    }
}
