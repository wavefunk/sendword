use sendword::config::AppConfig;

#[tokio::main]
async fn main() -> eyre::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "sendword=debug,tower_http=debug".parse().unwrap()),
        )
        .init();

    let config = AppConfig::load()?;
    tracing::info!(
        bind = %config.server.bind,
        port = config.server.port,
        hooks = config.hooks.len(),
        "config loaded"
    );

    let db = sendword::db::Db::new(&config.database).await?;
    db.migrate().await?;
    tracing::info!("database ready");

    Ok(())
}
