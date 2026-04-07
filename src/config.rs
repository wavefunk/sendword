use serde::Deserialize;
use std::collections::HashMap;
use std::time::Duration;

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
