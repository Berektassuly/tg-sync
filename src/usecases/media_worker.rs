//! Async task: reads MediaReference from mpsc channel and downloads files.
//!
//! Runs concurrently with text sync. Uses TgGateway and rate limiting.

use crate::domain::{DomainError, MediaReference};
use crate::ports::TgGateway;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::sleep;
use tracing::{debug, error, info};

/// Maximum concurrent media downloads.
const MAX_CONCURRENT: usize = 3;

/// Maximum retry attempts for a single media download.
const MAX_RETRIES: u32 = 3;

/// Base delay in seconds for linear backoff (sleep = retry_count * BASE_BACKOFF_SECS).
const BASE_BACKOFF_SECS: u64 = 2;

/// Media worker. Consumes channel and downloads via TgGateway.
pub struct MediaWorker {
    tg: Arc<dyn TgGateway>,
    rx: mpsc::Receiver<MediaReference>,
    output_dir: PathBuf,
}

impl MediaWorker {
    pub fn new(
        tg: Arc<dyn TgGateway>,
        rx: mpsc::Receiver<MediaReference>,
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

        let mut last_error = None;
        for attempt in 0..=MAX_RETRIES {
            match tg.download_media(media_ref, &dest).await {
                Ok(()) => return Ok(()),
                Err(e) => {
                    last_error = Some(e);
                    if attempt < MAX_RETRIES {
                        let delay_secs = (attempt + 1) as u64 * BASE_BACKOFF_SECS;
                        debug!(
                            chat_id = media_ref.chat_id,
                            msg_id = media_ref.message_id,
                            attempt = attempt + 1,
                            max_retries = MAX_RETRIES,
                            delay_secs,
                            error = %last_error.as_ref().unwrap(),
                            "download failed, retrying after backoff"
                        );
                        sleep(Duration::from_secs(delay_secs)).await;
                    }
                }
            }
        }

        let err = last_error.expect("last_error set in loop");
        error!(
            chat_id = media_ref.chat_id,
            msg_id = media_ref.message_id,
            file = %filename,
            error = %err,
            "Max retries exceeded for {}",
            filename
        );
        Err(err)
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
