use clap::{Parser, Subcommand};
use sendword::config::AppConfig;

#[derive(Parser)]
#[command(name = "sendword", about = "HTTP webhook to command runner sidecar")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Start the web server (default)
    Serve,
    /// User management commands
    User {
        #[command(subcommand)]
        action: UserAction,
    },
}

#[derive(Subcommand)]
enum UserAction {
    /// Create a new user
    Create {
        /// Username (3-32 chars, alphanumeric and hyphens)
        #[arg(long)]
        username: String,
    },
}

#[tokio::main]
async fn main() -> eyre::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        None | Some(Command::Serve) => serve().await,
        Some(Command::User { action }) => match action {
            UserAction::Create { username } => user_create(&username).await,
        },
    }
}

async fn serve() -> eyre::Result<()> {
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

    sendword::barriers::recover_barriers(db.pool()).await;
    tracing::info!("barrier state recovered");

    let _session_sweep = sendword::tasks::spawn_session_sweep(db.pool().clone());
    tracing::info!("session sweep task started");

    let templates = sendword::templates::Templates::new(
        sendword::templates::Templates::default_dir(),
    );
    tracing::info!("templates loaded");

    let state = sendword::server::AppState::new(config, "sendword.toml", db, templates);

    let _approval_sweep = sendword::tasks::spawn_approval_sweep(
        state.db.pool().clone(),
        std::sync::Arc::clone(&state),
    );
    tracing::info!("approval sweep task started");

    sendword::server::run(state).await?;

    Ok(())
}

async fn user_create(username: &str) -> eyre::Result<()> {
    // Validate username before prompting for password
    if let Err(msg) = sendword::models::user::validate_username(username) {
        eprintln!("error: {msg}");
        std::process::exit(1);
    }

    // Prompt for password interactively
    let password = rpassword::prompt_password("Password: ")?;
    if password.is_empty() {
        eprintln!("error: password must not be empty");
        std::process::exit(1);
    }

    let confirm = rpassword::prompt_password("Confirm password: ")?;
    if password != confirm {
        eprintln!("error: passwords do not match");
        std::process::exit(1);
    }

    // Load config and connect to database
    let config = AppConfig::load()?;
    let db = sendword::db::Db::new(&config.database).await?;
    db.migrate().await?;

    // Create user
    match sendword::models::user::create(db.pool(), username, &password).await {
        Ok(user) => {
            eprintln!("user '{}' created (id: {})", user.username, user.id);
        }
        Err(sendword::error::DbError::Conflict(msg)) => {
            eprintln!("error: {msg}");
            std::process::exit(1);
        }
        Err(e) => {
            return Err(e.into());
        }
    }

    Ok(())
}
