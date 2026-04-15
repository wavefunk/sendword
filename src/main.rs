use std::sync::Arc;

use clap::{Parser, Subcommand};

use allowthem_core::{AllowThemBuilder, Email, EmbeddedAuthClient};

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
    /// Export current config as JSON to stdout
    Export,
    /// Import config from a JSON file, validate, write to sendword.toml, and reload
    Import {
        /// Path to the JSON config file to import
        path: std::path::PathBuf,
    },
    /// User management commands
    User {
        #[command(subcommand)]
        action: UserAction,
    },
    /// Backup management commands
    Backup {
        #[command(subcommand)]
        action: BackupAction,
    },
    /// Restore from a backup
    Restore {
        /// S3 object key of the backup to restore
        #[arg(long)]
        from: String,
        /// Directory to extract the backup into
        #[arg(long, default_value = "restored")]
        output: std::path::PathBuf,
    },
}

#[derive(Subcommand)]
enum UserAction {
    /// Create a new user
    Create {
        /// Email address for the new user
        #[arg(long)]
        email: String,
    },
}

#[derive(Subcommand)]
enum BackupAction {
    /// Create a backup and upload to S3
    Create,
    /// List available backups
    List,
}

#[tokio::main]
async fn main() -> eyre::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        None | Some(Command::Serve) => serve().await,
        Some(Command::Export) => config_export().await,
        Some(Command::Import { path }) => config_import(&path).await,
        Some(Command::User { action }) => match action {
            UserAction::Create { email } => user_create(&email).await,
        },
        Some(Command::Backup { action }) => match action {
            BackupAction::Create => backup_create().await,
            BackupAction::List => backup_list().await,
        },
        Some(Command::Restore { from, output }) => backup_restore(&from, &output).await,
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

    let session_ttl = chrono::Duration::from_std(config.auth.session_lifetime)
        .unwrap_or(chrono::Duration::hours(24));
    let ath = AllowThemBuilder::with_pool(db.pool().clone())
        .session_ttl(session_ttl)
        .cookie_secure(config.auth.secure_cookie)
        .build()
        .await?;
    let auth_client = Arc::new(EmbeddedAuthClient::new(ath.clone(), "/login"));
    tracing::info!("allowthem auth ready");

    let templates =
        sendword::templates::Templates::new(sendword::templates::Templates::default_dir());
    tracing::info!("templates loaded");

    let state =
        sendword::server::AppState::new(config, "sendword.toml", db, templates, ath, auth_client);

    let _rate_limit_sweep = sendword::tasks::spawn_rate_limit_sweep(state.db.pool().clone());
    tracing::info!("rate limit sweep task started");

    let _approval_sweep = sendword::tasks::spawn_approval_sweep(
        state.db.pool().clone(),
        std::sync::Arc::clone(&state),
    );
    tracing::info!("approval sweep task started");

    if state
        .config
        .load()
        .backup
        .as_ref()
        .and_then(|b| b.schedule.as_ref())
        .is_some()
    {
        let _backup_scheduler =
            sendword::backup::scheduler::spawn_backup_scheduler(std::sync::Arc::clone(&state));
        tracing::info!("backup scheduler started");
    }

    sendword::server::run(state).await?;

    Ok(())
}

async fn config_export() -> eyre::Result<()> {
    let config = AppConfig::load()?;
    let json = serde_json::to_string_pretty(&config)?;
    println!("{json}");
    Ok(())
}

async fn config_import(path: &std::path::Path) -> eyre::Result<()> {
    let contents = std::fs::read_to_string(path)?;
    let config: AppConfig = serde_json::from_str(&contents)?;
    if let Err(e) = config.validate() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
    let toml_str = toml_edit::ser::to_string_pretty(&config)?;
    std::fs::write("sendword.toml", toml_str.as_bytes())?;
    eprintln!("config imported and written to sendword.toml");
    Ok(())
}

async fn user_create(email_str: &str) -> eyre::Result<()> {
    let email = match Email::new(email_str.to_owned()) {
        Ok(e) => e,
        Err(_) => {
            eprintln!("error: invalid email address");
            std::process::exit(1);
        }
    };

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

    let ath = AllowThemBuilder::with_pool(db.pool().clone())
        .build()
        .await?;

    match ath.db().create_user(email, &password, None).await {
        Ok(user) => {
            eprintln!("user '{}' created (id: {})", user.email.as_str(), user.id);
        }
        Err(allowthem_core::AuthError::Conflict(msg)) => {
            eprintln!("error: {msg}");
            std::process::exit(1);
        }
        Err(e) => {
            return Err(e.into());
        }
    }

    Ok(())
}

async fn backup_create() -> eyre::Result<()> {
    let config = AppConfig::load()?;
    let backup_config = config
        .backup
        .as_ref()
        .ok_or_else(|| eyre::eyre!("backup is not configured in sendword.toml"))?;

    let db = sendword::db::Db::new(&config.database).await?;
    db.migrate().await?;

    let config_path = std::path::Path::new("sendword.toml");
    match sendword::backup::create_backup(db.pool(), backup_config, config_path).await {
        Ok(key) => {
            eprintln!("backup created: {key}");
        }
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }

    Ok(())
}

async fn backup_list() -> eyre::Result<()> {
    let config = AppConfig::load()?;
    let backup_config = config
        .backup
        .as_ref()
        .ok_or_else(|| eyre::eyre!("backup is not configured in sendword.toml"))?;

    match sendword::backup::list_backups(backup_config).await {
        Ok(entries) => {
            if entries.is_empty() {
                eprintln!("no backups found");
            } else {
                for entry in &entries {
                    println!("{}\t{}\t{}", entry.last_modified, entry.size, entry.key);
                }
            }
        }
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }

    Ok(())
}

async fn backup_restore(key: &str, output: &std::path::Path) -> eyre::Result<()> {
    let config = AppConfig::load()?;
    let backup_config = config
        .backup
        .as_ref()
        .ok_or_else(|| eyre::eyre!("backup is not configured in sendword.toml"))?;

    match sendword::backup::restore_backup(backup_config, key, output).await {
        Ok(()) => {
            eprintln!("backup extracted to: {}", output.display());
            eprintln!(
                "apply manually: copy sendword.toml and sendword.db from the output directory"
            );
        }
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }

    Ok(())
}
