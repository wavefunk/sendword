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

    tokio::fs::create_dir_all(&config.scripts.dir).await?;
    tracing::info!(dir = %config.scripts.dir, "scripts directory ready");

    let db = sendword::db::Db::new(&config.database).await?;
    db.migrate().await?;
    tracing::info!("database ready");

    let _sweep_handle = sendword::tasks::spawn_session_sweep(db.pool().clone());
    tracing::info!("session sweep task started");

    let templates = sendword::templates::Templates::new(
        sendword::templates::Templates::default_dir(),
    );
    tracing::info!("templates loaded");

    let state = sendword::server::AppState::new(config, db, templates);
    sendword::server::run(state).await?;

    Ok(())
}
