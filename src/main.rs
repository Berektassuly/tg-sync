//! Wiring & DI. Entry point: bootstrap adapters, inject into services, run UI.
//! No business logic here; authentication is delegated to AuthService.

use std::path::PathBuf;
use std::sync::Arc;
use tg_sync::adapters::persistence::{sqlite_repo::SqliteRepo, state_json::StateJson};
use tg_sync::adapters::telegram::{auth_adapter::GrammersAuthAdapter, client::GrammersTgGateway};
use tg_sync::adapters::tools::chatpack::ChatpackProcessor;
use tg_sync::adapters::ui::tui::TuiInputPort;
use tg_sync::ports::{AuthPort, InputPort, RepoPort, StatePort, TgGateway};
use tg_sync::usecases::{AuthService, MediaWorker, SyncService};
use tokio::sync::mpsc;
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
    let session_path = cfg
        .session_path
        .as_deref()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(&data_dir).join("session.db"));

    // --- Telegram client (shared between auth and gateway) ---
    let tg_client = create_telegram_client(&cfg, &session_path).await?;
    let tg_client = Arc::new(tokio::sync::Mutex::new(tg_client));

    // --- Auth: adapter + service, then run flow ---
    let auth_adapter: Arc<dyn AuthPort> =
        Arc::new(GrammersAuthAdapter::new(Arc::clone(&tg_client)));
    let auth_service = AuthService::new(auth_adapter, api_hash);
    auth_service
        .run_auth_flow()
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))?;

    // --- Gateway (same client as auth) ---
    let tg: Arc<dyn TgGateway> = Arc::new(GrammersTgGateway::new(tg_client, cfg.export_delay_ms));

    let repo: Arc<dyn RepoPort> = Arc::new(
        SqliteRepo::connect(&data_dir)
            .await
            .map_err(|e| anyhow::anyhow!("{}", e))?,
    );
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

/// Create grammers Client with persistent session storage.
/// Loads existing session from `session_path` if present; otherwise a new session is created
/// and will be saved after login. Requires TG_SYNC_API_ID (and TG_SYNC_API_HASH for login).
async fn create_telegram_client(
    cfg: &tg_sync::shared::config::AppConfig,
    session_path: &std::path::Path,
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

    let session = tg_sync::adapters::telegram::session::open_file_session(session_path).await?;
    let session = Arc::new(session);
    let pool = grammers_client::SenderPool::new(session, api_id);
    let handle = pool.handle.clone();
    tokio::spawn(async move {
        pool.runner.run().await;
    });
    let client = grammers_client::Client::new(handle);

    Ok(client)
}
