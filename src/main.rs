//! Wiring & DI. Entry point: bootstrap adapters, inject into services, run UI.
//! No business logic here; authentication is delegated to AuthService.

use dotenv::dotenv;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tg_sync::adapters::ai::{MockAiAdapter, OpenAiAdapter};
use tg_sync::adapters::integrations::trello::TrelloAdapter;
use tg_sync::adapters::persistence::{sqlite_repo::SqliteRepo, state_json::StateJson};
use tg_sync::adapters::telegram::{auth_adapter::GrammersAuthAdapter, client::GrammersTgGateway};
use tg_sync::adapters::tools::chatpack::ChatpackProcessor;
use tg_sync::adapters::ui::tui::TuiInputPort;
use tg_sync::ports::{
    AiPort, AnalysisLogPort, AuthPort, InputPort, RepoPort, StatePort, TaskTrackerPort, TgGateway,
};
use tg_sync::shared::config::DEFAULT_MEDIA_QUEUE_SIZE;
use tg_sync::usecases::{AnalysisService, AuthService, MediaWorker, SyncService, WatcherService};
use tokio::sync::mpsc;
use tracing::{info, warn};

/// Bounded channel capacity for media refs. Producer (sync) blocks on send().await when full (backpressure).
const CHANNEL_CAPACITY: usize = DEFAULT_MEDIA_QUEUE_SIZE;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let env_loaded = dotenv();
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .with(tracing_subscriber::fmt::layer())
        .init();

    match &env_loaded {
        Ok(path) => info!(path = %path.display(), "loaded .env"),
        Err(_) => info!(cwd = %cwd.display(), "no .env found (check CWD)"),
    }

    tg_sync::adapters::ui::init_ui();

    let cfg = tg_sync::shared::config::AppConfig::load().unwrap_or_default();
    if std::env::var("TG_SYNC_AI_API_KEY").is_ok() {
        info!("TG_SYNC_AI_API_KEY is set (env)");
    } else {
        info!("TG_SYNC_AI_API_KEY is not set in env");
    }
    let api_hash = cfg
        .api_hash
        .clone()
        .or_else(|| std::env::var("TG_SYNC_API_HASH").ok())
        .unwrap_or_default();
    if api_hash.is_empty() {
        anyhow::bail!("Set TG_SYNC_API_HASH (env or .env). Get from https://my.telegram.org");
    }

    let data_dir = cfg.data_dir.as_deref().unwrap_or("./data").to_string();
    let data_path = PathBuf::from(&data_dir);
    let data_dir_abs = data_path
        .canonicalize()
        .unwrap_or_else(|_| data_path.clone());
    info!(
        path = %data_dir_abs.display(),
        "data directory: {}",
        data_dir_abs.display()
    );
    let state_path = data_path.join("state.json");
    let session_path = cfg
        .session_path
        .as_deref()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("./session.db"));

    // --- Telegram client (cloned for auth and gateway; same session, no global lock) ---
    let tg_client = create_telegram_client(&cfg, &session_path).await?;

    // --- Auth: adapter + service, then run flow ---
    let auth_adapter: Arc<dyn AuthPort> = Arc::new(GrammersAuthAdapter::new(tg_client.clone()));
    let auth_service = AuthService::new(auth_adapter, api_hash);
    auth_service
        .run_auth_flow()
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))?;

    // --- Gateway (clone of same client; fetch_messages and download_media can run concurrently) ---
    let tg: Arc<dyn TgGateway> = Arc::new(GrammersTgGateway::new(tg_client, cfg.export_delay_ms));

    // Audit §2.4: Use SqliteRepo for ACID compliance, WAL mode, and EntityRegistry support.
    let sqlite_repo = Arc::new(
        SqliteRepo::connect(&data_path)
            .await
            .map_err(|e| anyhow::anyhow!("SQLite connect failed: {}", e))?,
    );
    let repo: Arc<dyn RepoPort> = Arc::clone(&sqlite_repo) as Arc<dyn RepoPort>;
    let analysis_log: Arc<dyn AnalysisLogPort> =
        Arc::clone(&sqlite_repo) as Arc<dyn AnalysisLogPort>;
    let state_impl = StateJson::new(&state_path);
    state_impl
        .load()
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))?;
    let state: Arc<dyn StatePort> = Arc::new(state_impl);

    let _processor = Arc::new(ChatpackProcessor::new(None::<&str>));

    // --- Media pipeline: bounded channel for backpressure (producer blocks when full) ---
    let media_queue_size = cfg.media_queue_size.unwrap_or(CHANNEL_CAPACITY);
    info!(
        media_queue_size,
        "media queue buffer: {} (backpressure)", media_queue_size
    );
    let (media_tx, media_rx) = mpsc::channel(media_queue_size);
    let media_dir = data_path.join("media");
    tokio::fs::create_dir_all(&media_dir)
        .await
        .map_err(|e| anyhow::anyhow!("create media dir: {}", e))?;
    let media_worker = MediaWorker::new(Arc::clone(&tg), media_rx, media_dir);
    tokio::spawn(async move {
        media_worker.run().await;
    });

    // --- Sync rate limit (SYNC_DELAY_MS, default 500ms) ---
    let sync_delay_ms = cfg.sync_delay_ms_or_default();
    let sync_delay = Duration::from_millis(sync_delay_ms);
    info!(
        sync_delay_ms,
        "sync rate limit: {} ms between batches", sync_delay_ms
    );

    // --- Services ---
    let sync_service = Arc::new(SyncService::new(
        Arc::clone(&tg),
        Arc::clone(&repo),
        Arc::clone(&state),
        media_tx,
        sync_delay,
    ));

    let watcher_cycle_secs = cfg.watcher_cycle_secs_or_default();
    let watcher_service = Arc::new(WatcherService::new(
        Arc::clone(&tg),
        Arc::clone(&repo),
        Arc::clone(&sync_service),
        Duration::from_secs(watcher_cycle_secs),
    ));

    // --- AI Analysis Service ---
    let ai_adapter: Arc<dyn AiPort> = if cfg.is_ai_configured() {
        info!(
            model = %cfg.ai_model_or_default(),
            url = %cfg.ai_api_url_or_default(),
            "AI analysis enabled with OpenAI adapter"
        );
        Arc::new(OpenAiAdapter::new(
            cfg.ai_api_url_or_default(),
            cfg.ai_api_key().unwrap_or_default(),
            cfg.ai_model_or_default(),
        ))
    } else {
        warn!("TG_SYNC_AI_API_KEY not set, using mock AI adapter");
        Arc::new(MockAiAdapter::new())
    };

    let reports_dir = data_path.join("reports");
    let task_tracker: Option<Arc<dyn TaskTrackerPort>> = if cfg.is_trello_configured() {
        info!("Trello task tracker enabled (TRELLO_KEY, TRELLO_TOKEN, TRELLO_LIST_ID)");
        Some(Arc::new(TrelloAdapter::new(
            cfg.trello_key().unwrap_or_default(),
            cfg.trello_token().unwrap_or_default(),
            cfg.trello_board_id().unwrap_or_default(),
            cfg.trello_list_id().unwrap_or_default(),
        )))
    } else {
        None
    };
    let analysis_service = Arc::new(AnalysisService::new(
        ai_adapter,
        analysis_log,
        reports_dir,
        task_tracker,
    ));

    let input_port: Arc<dyn InputPort> = Arc::new(TuiInputPort::new(
        Arc::clone(&tg),
        Arc::clone(&repo),
        Arc::clone(&sync_service),
        Arc::clone(&watcher_service),
        Arc::clone(&analysis_service),
    ));

    // --- Run (main menu -> Full Backup / Watcher / AI Analysis) ---
    input_port
        .run()
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
