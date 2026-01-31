//! Wiring & DI. Setup adapters, inject into services, run UI.

use std::path::PathBuf;
use std::sync::Arc;
use tg_sync::adapters::persistence::{fs_repo::FsRepo, state_json::StateJson};
use tg_sync::adapters::telegram::client::GrammersTgGateway;
use tg_sync::adapters::tools::chatpack::ChatpackProcessor;
use tg_sync::adapters::ui::tui::TuiInputPort;
use tg_sync::ports::{InputPort, RepoPort, StatePort, TgGateway};
use tg_sync::usecases::{MediaWorker, SyncService};
use tokio::sync::mpsc;
use tracing::{info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .with(tracing_subscriber::fmt::layer())
        .init();

    tg_sync::adapters::ui::init_ui();

    let cfg = tg_sync::shared::config::AppConfig::load().unwrap_or_default();
    let api_hash = cfg
        .api_hash
        .clone()
        .or_else(|| std::env::var("TG_SYNC_API_HASH").ok())
        .unwrap_or_default();
    if api_hash.is_empty() {
        anyhow::bail!("Set TG_SYNC_API_HASH (env or .env). Get from https://my.telegram.org");
    }

    let data_dir = cfg.data_dir.as_deref().unwrap_or("./data").to_string();
    let state_path = PathBuf::from(&data_dir).join("state.json");

    // --- Adapters ---
    let tg_client = create_telegram_client(&cfg).await?;
    ensure_authorized(&tg_client, &api_hash).await?;
    let tg: Arc<dyn TgGateway> = Arc::new(GrammersTgGateway::new(tg_client, cfg.export_delay_ms));

    let repo: Arc<dyn RepoPort> = Arc::new(FsRepo::new(&data_dir));
    let state_impl = StateJson::new(&state_path);
    state_impl
        .load()
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))?;
    let state: Arc<dyn StatePort> = Arc::new(state_impl);

    let _processor = Arc::new(ChatpackProcessor::new(None::<&str>));

    // --- Media pipeline ---
    let (media_tx, media_rx) = mpsc::unbounded_channel();
    let media_dir = PathBuf::from(&data_dir).join("media");
    tokio::fs::create_dir_all(&media_dir)
        .await
        .map_err(|e| anyhow::anyhow!("create media dir: {}", e))?;
    let media_worker = MediaWorker::new(Arc::clone(&tg), media_rx, media_dir);
    tokio::spawn(async move {
        media_worker.run().await;
    });

    // --- Services ---
    let sync_service = Arc::new(SyncService::new(
        Arc::clone(&tg),
        Arc::clone(&repo),
        Arc::clone(&state),
        media_tx,
    ));

    let input_port: Arc<dyn InputPort> = Arc::new(TuiInputPort::new(
        Arc::clone(&tg),
        Arc::clone(&sync_service),
    ));

    // --- Run ---
    input_port
        .run_sync()
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))?;

    Ok(())
}

/// Create grammers Client. Uses MemorySession (session not persisted by default).
/// Requires TG_SYNC_API_ID (and TG_SYNC_API_HASH for login).
async fn create_telegram_client(
    cfg: &tg_sync::shared::config::AppConfig,
) -> anyhow::Result<grammers_client::Client> {
    let api_id = cfg
        .api_id
        .or_else(|| {
            std::env::var("TG_SYNC_API_ID")
                .ok()
                .and_then(|s| s.parse().ok())
        })
        .unwrap_or(0);

    if api_id == 0 {
        anyhow::bail!(
            "Set TG_SYNC_API_ID (and TG_SYNC_API_HASH) in .env. Get from https://my.telegram.org"
        );
    }

    let session = Arc::new(grammers_session::storages::MemorySession::default());
    let pool = grammers_client::SenderPool::new(session, api_id);
    let handle = pool.handle.clone();
    tokio::spawn(async move {
        pool.runner.run().await;
    });
    let client = grammers_client::Client::new(handle);

    Ok(client)
}

/// Ensure the client is logged in. If not, run phone + code (and 2FA if needed).
async fn ensure_authorized(client: &grammers_client::Client, api_hash: &str) -> anyhow::Result<()> {
    if client
        .is_authorized()
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))?
    {
        info!("Already authorized");
        return Ok(());
    }

    warn!("Not authorized. Running login flow (phone + code from Telegram app/SMS).");
    let phone = inquire::Text::new("Phone number (e.g. +1234567890):")
        .prompt()
        .map_err(|e| anyhow::anyhow!("input: {}", e))?;
    let token = client
        .request_login_code(&phone, api_hash)
        .await
        .map_err(|e| anyhow::anyhow!("request_login_code: {}", e))?;
    let code = inquire::Text::new("Login code from Telegram:")
        .prompt()
        .map_err(|e| anyhow::anyhow!("input: {}", e))?;

    match client.sign_in(&token, &code).await {
        Ok(user) => {
            let name = user.first_name().unwrap_or("user");
            info!("Signed in as {}", name);
            Ok(())
        }
        Err(grammers_client::SignInError::PasswordRequired(password_token)) => {
            let hint = password_token.hint().unwrap_or("(no hint)");
            let prompt = format!("2FA password (hint: {}):", hint);
            let password = inquire::Password::new(&prompt)
                .prompt()
                .map_err(|e| anyhow::anyhow!("input: {}", e))?;
            let _user = client
                .check_password(password_token, password.as_bytes())
                .await
                .map_err(|e| anyhow::anyhow!("check_password: {}", e))?;
            info!("Signed in (2FA completed)");
            Ok(())
        }
        Err(grammers_client::SignInError::InvalidCode) => {
            anyhow::bail!("Invalid login code. Run again and enter the correct code.")
        }
        Err(grammers_client::SignInError::SignUpRequired) => {
            anyhow::bail!(
                "Sign-up required. Create an account with the official Telegram app first."
            )
        }
        Err(e) => Err(anyhow::anyhow!("sign in: {}", e)),
    }
}
