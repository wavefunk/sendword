use std::str::FromStr;
use std::sync::Arc;

use crate::server::AppState;

/// Spawn a background task that runs backups on the configured cron schedule.
///
/// The task checks the config each iteration so that schedule changes take
/// effect without a server restart (config is hot-reloaded via ArcSwap).
pub fn spawn_backup_scheduler(state: Arc<AppState>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let config = state.config.load();
            let Some(backup_config) = &config.backup else {
                drop(config);
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                continue;
            };

            let Some(schedule_str) = &backup_config.schedule else {
                drop(config);
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                continue;
            };

            let schedule = match cron::Schedule::from_str(schedule_str) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(schedule = %schedule_str, error = %e, "invalid backup schedule, skipping");
                    drop(config);
                    tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                    continue;
                }
            };

            let next = schedule.upcoming(chrono::Utc).next();
            drop(config);

            let Some(next_time) = next else {
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                continue;
            };

            let now = chrono::Utc::now();
            let delay = next_time.signed_duration_since(now);
            if delay.num_milliseconds() > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(
                    delay.num_milliseconds() as u64
                ))
                .await;
            }

            // Re-load config at execution time in case it changed
            let config = state.config.load();
            let Some(backup_config) = config.backup.clone() else {
                continue;
            };

            let pool = state.db.pool().clone();
            // Determine config path from config_writer
            let config_path = state.config_writer.path().to_owned();

            match super::create_backup(&pool, &backup_config, &config_path).await {
                Ok(key) => {
                    tracing::info!(key = %key, "scheduled backup completed");
                    if let Err(e) = super::apply_retention(&backup_config).await {
                        tracing::warn!(error = %e, "retention policy application failed");
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "scheduled backup failed");
                }
            }
        }
    })
}
