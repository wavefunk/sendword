use reqwest::Client;
use sqlx::SqlitePool;

use super::{ExecutionContext, ExecutionResult};

/// Run an HTTP executor. Placeholder until M6 implementation.
pub async fn run_http(
    _pool: &SqlitePool,
    _ctx: &ExecutionContext,
    _client: &Client,
) -> ExecutionResult {
    unimplemented!("HTTP executor is not yet implemented (M6)")
}
