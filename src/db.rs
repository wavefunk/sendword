use crate::config::DatabaseConfig;
use crate::error::{DbError, DbResult};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
use std::path::Path;
use std::str::FromStr;

#[derive(Clone, Debug)]
pub struct Db {
    pool: SqlitePool,
}

impl Db {
    pub async fn new(config: &DatabaseConfig) -> DbResult<Self> {
        // Ensure the parent directory exists
        if let Some(parent) = Path::new(&config.path).parent() {
            std::fs::create_dir_all(parent)?;
        }

        let url = if config.path.starts_with("sqlite:") {
            config.path.clone()
        } else {
            format!("sqlite://{}", config.path)
        };

        let options = SqliteConnectOptions::from_str(&url)?
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .foreign_keys(true)
            .synchronous(sqlx::sqlite::SqliteSynchronous::Normal)
            .busy_timeout(std::time::Duration::from_millis(5000));

        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(options)
            .await?;

        Ok(Self { pool })
    }

    pub async fn new_in_memory() -> DbResult<Self> {
        let options = SqliteConnectOptions::from_str("sqlite::memory:")?
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .foreign_keys(true)
            .synchronous(sqlx::sqlite::SqliteSynchronous::Normal);

        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await?;

        Ok(Self { pool })
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    pub async fn migrate(&self) -> DbResult<()> {
        sqlx::migrate!("./migrations")
            .run(&self.pool)
            .await
            .map_err(|e| DbError::Migration(e.to_string()))?;
        Ok(())
    }
}
