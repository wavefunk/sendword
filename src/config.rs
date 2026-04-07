use figment::{
    providers::{Env, Format, Json, Toml},
    Figment,
};
use serde::Deserialize;
use std::collections::HashMap;
use std::time::Duration;

// --- Error type ---

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("config loading failed: {0}")]
    Figment(#[from] figment::Error),

    #[error("config validation failed:\n{0}")]
    Validation(String),
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

// --- Config types ---

#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub database: DatabaseConfig,
    #[serde(default)]
    pub logs: LogsConfig,
    #[serde(default)]
    pub defaults: DefaultsConfig,
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

        let config: AppConfig = figment.extract()?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<(), ConfigError> {
        Ok(())
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            server: ServerConfig::default(),
            database: DatabaseConfig::default(),
            logs: LogsConfig::default(),
            defaults: DefaultsConfig::default(),
            hooks: Vec::new(),
        }
    }
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

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExecutorConfig {
    Shell { command: String },
}

#[derive(Debug, Clone, Deserialize)]
pub struct HookConfig {
    pub name: String,
    pub slug: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub executor: ExecutorConfig,
    #[serde(default)]
    pub env: HashMap<String, String>,
    pub cwd: Option<String>,
    #[serde(default, with = "humantime_serde::option")]
    pub timeout: Option<Duration>,
    pub retries: Option<RetryConfig>,
    pub rate_limit: Option<RateLimitConfig>,
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

            let ExecutorConfig::Shell { command } = &hook.executor;
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
}
