//! Async task: reads MediaReference from mpsc channel and downloads files.
//!
//! Runs concurrently with text sync. Uses TgGateway and rate limiting.

use crate::domain::{DomainError, MediaReference};
use crate::ports::TgGateway;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, error, info};

/// Maximum concurrent media downloads.
const MAX_CONCURRENT: usize = 3;

/// Media worker. Consumes channel and downloads via TgGateway.
pub struct MediaWorker {
    tg: Arc<dyn TgGateway>,
    rx: mpsc::UnboundedReceiver<MediaReference>,
    output_dir: PathBuf,
}

impl MediaWorker {
    pub fn new(
        tg: Arc<dyn TgGateway>,
        rx: mpsc::UnboundedReceiver<MediaReference>,
        output_dir: PathBuf,
    ) -> Self {
        Self { tg, rx, output_dir }
    }

    /// Run the worker. Processes until channel is closed.
    pub async fn run(mut self) {
        let semaphore = Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT));

        while let Some(media_ref) = self.rx.recv().await {
            let sem = Arc::clone(&semaphore);
            let tg = Arc::clone(&self.tg);
            let output_dir = self.output_dir.clone();

            tokio::spawn(async move {
                let _permit = sem.acquire().await.expect("semaphore closed");
                if let Err(e) = Self::download_one(&*tg, &media_ref, &output_dir).await {
                    error!(chat_id = media_ref.chat_id, msg_id = media_ref.message_id, error = %e, "media download failed");
                } else {
                    debug!(
                        chat_id = media_ref.chat_id,
                        msg_id = media_ref.message_id,
                        "media downloaded"
                    );
                }
            });
        }

        info!("media worker finished (channel closed)");
    }

    async fn download_one(
        tg: &dyn TgGateway,
        media_ref: &MediaReference,
        base: &std::path::Path,
    ) -> Result<(), DomainError> {
        let ext = extension_for_media_type(media_ref.media_type);
        let filename = format!("{}_{}.{}", media_ref.chat_id, media_ref.message_id, ext);
        let dest = base.join(&filename);

        if tokio::fs::try_exists(&dest).await.unwrap_or(false) {
            debug!(path = %dest.display(), "File already exists: skipping download");
            return Ok(());
        }

        tg.download_media(media_ref, &dest).await
    }
}

fn extension_for_media_type(media_type: crate::domain::MediaType) -> &'static str {
    use crate::domain::MediaType;
    match media_type {
        MediaType::Photo => "jpg",
        MediaType::Video => "mp4",
        MediaType::Document => "bin",
        MediaType::Audio => "ogg",
        MediaType::Voice => "ogg",
        MediaType::Sticker => "webp",
        MediaType::Animation => "mp4",
        MediaType::Other => "bin",
    }
}
