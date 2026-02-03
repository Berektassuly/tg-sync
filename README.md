<div align="center">

# tg-sync

**High-Performance, Resilient Telegram Archiving System**

[![Rust](https://img.shields.io/badge/rust-1.75%2B-orange?logo=rust)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

A CLI userbot to incrementally backup Telegram chats and groups (with media), run AI-powered weekly digests, and optionally push action items to Trello. Built for high throughput and ACID-safe persistence.

</div>

---

## Overview

`tg-sync` is not a simple chatbot script. It is an enterprise-grade archival solution designed to handle **massive datasets** without crashing (OOM) or being banned (FLOOD_WAIT). Built on a **Hexagonal Architecture**, it cleanly separates business logic from infrastructure, making the system testable, maintainable, and resilient.

The system uses the MTProto protocol directly via the [grammers](https://github.com/Lonami/grammers) library, enabling full control over API interactions, rate limiting, and session management.

### Modes (TUI Menu)

On startup you choose one of:

| Mode | Description |
|------|-------------|
| **Full Backup** | Sync all (non-blacklisted) dialogs: fetch message history, persist to SQLite, download media to disk. |
| **Manage Blacklist** | Exclude specific chats from backup (dialogs in the blacklist are skipped during sync). |
| **Watcher / Daemon** | Periodically check dialogs and run incremental sync in a loop (configurable cycle, default 600 s). |
| **AI Analysis** | Generate weekly digest reports per chat using an LLM (OpenAI or Ollama). Optionally create Trello cards for action items. |

---

## Key Features

### Memory Safety

- **Bounded Channels with Backpressure**: The sync producer and media consumer communicate via a `tokio::sync::mpsc` channel with a configurable capacity (default: `1000`). When the channel is full, `send().await` yields, naturally throttling the producer. This prevents unbounded memory growth regardless of download speed.
- **Semaphore-Controlled Concurrency**: Media downloads are limited to 3 concurrent operations via a `tokio::sync::Semaphore`, preventing resource exhaustion.

### Resilience

- **Intelligent FLOOD_WAIT Handling**: The system distinguishes between short and long API rate limits:
  - **Short waits (< 60s)**: The worker thread sleeps and retries automatically.
  - **Long waits (≥ 60s)**: Returns a `FloodWait` error, allowing the job scheduler to reschedule without blocking the thread.
- **Automatic Retry with Backoff**: Media downloads retry up to 3 times with linear backoff (2s, 4s, 6s) before failing permanently.
- **Persistent Peer Caching**: An `entity_registry` table caches `access_hash` values per peer, eliminating redundant `getDialogs` calls which are a primary cause of `FLOOD_WAIT` errors.

### Data Integrity

- **SQLite with WAL Mode**: All messages are persisted in a single `messages.db` file using SQLite's Write-Ahead Logging for concurrent read/write access and crash resilience.
- **Atomic State Writes**: The `state.json` file (tracking `last_message_id` per chat) uses a **write-replace pattern**: data is written to a `.tmp` file, `sync_all()` flushes to disk, and an atomic `rename()` replaces the original. This prevents data loss during unexpected termination.
- **Transactional Batch Saves**: Message batches are inserted within a SQLite transaction. Either the entire batch commits, or nothing does.

### Efficiency

- **Hybrid Schema (JSONB + SQL Columns)**: Raw media metadata is stored as `media_json` (flexible, future-proof) while core fields (`id`, `date`, `text`, `from_user_id`) are indexed SQL columns for fast queries.
- **Incremental Sync**: Only messages newer than the last checkpoint are fetched. Client-side boundary enforcement ensures correctness even when the API ignores `min_id`/`max_id`.
- **Forward History Filling**: Paginates from newest to oldest, processing in forward order (oldest → newest) for consistent history.

### AI Analysis & Task Tracking

- **Weekly digest reports**: For each chat, unanalyzed weeks are sent to an LLM (OpenAI or Ollama). Reports are written as Markdown under `data/reports/` (e.g. `analysis_{chat_id}_{year}-{week}.md`) with summary, key topics, and action items.
- **Trello integration**: When `TRELLO_KEY`, `TRELLO_TOKEN`, and `TRELLO_LIST_ID` are set, action items from the AI analysis are created as cards on the given Trello list.
- **Mock adapter**: If `TG_SYNC_AI_API_KEY` is not set, the app uses a mock AI adapter so you can run the TUI and workflows without real LLM calls.

---

## Architecture

The application follows a **producer-consumer pipeline** with backpressure:

```
┌──────────────────────────────────────────────────────────────────────┐
│                           tg-sync Pipeline                           │
└──────────────────────────────────────────────────────────────────────┘

  ┌─────────────────┐                          ┌─────────────────────┐
  │   SyncService   │                          │    MediaWorker      │
  │    (Producer)   │                          │    (Consumer)       │
  │                 │    Bounded Channel       │                     │
  │  Fetches msgs   │──────[capacity: 1000]───►│  Downloads media    │
  │  from Telegram  │    mpsc::Sender.send()   │  to disk (3 conc.)  │
  │                 │    blocks when full      │                     │
  └────────┬────────┘                          └──────────┬──────────┘
           │                                              │
           │ Transactional Insert                         │ File I/O
           ▼                                              ▼
  ┌─────────────────┐                          ┌─────────────────────┐
  │  SQLite (WAL)   │                          │     data/media/     │
  │  messages.db    │                          │  {chat}_{msg}.ext   │
  └─────────────────┘                          └─────────────────────┘
           │
           │ Atomic Rename
           ▼
  ┌─────────────────┐
  │   state.json    │
  │  (checkpoints)  │
  └─────────────────┘
```

**Hexagonal Architecture Layers:**

| Layer | Responsibility | Key Files |
|-------|----------------|-----------|
| **Domain** | Pure business entities, error types | `entities.rs`, `errors.rs` |
| **Ports** | Abstract traits (inbound/outbound interfaces) | `inbound.rs`, `outbound.rs` |
| **Adapters** | Concrete implementations (Telegram, SQLite, TUI) | `telegram/`, `persistence/`, `ui/` |
| **Use Cases** | Business logic orchestration | `sync_service.rs`, `media_worker.rs`, `auth_service.rs`, `watcher_service.rs`, `analysis_service.rs` |

---

## Installation

### Prerequisites

- **Rust**: 1.75+ (stable) — [Install Rust](https://rustup.rs/)
- **SQLite**: The `libsql` crate bundles SQLite; no system-level installation required.

### Build

```bash
git clone https://github.com/Berektassuly/tg-sync.git
cd tg-sync
cargo build --release
```

The binary will be available at `target/release/tg-sync.exe` (Windows) or `target/release/tg-sync` (Unix).

---

## Configuration

### Environment Variables

Create a `.env` file in the project root (see `.env.example`):

```dotenv
# Required: Telegram API Credentials (https://my.telegram.org/apps)
TG_SYNC_API_ID=12345678
TG_SYNC_API_HASH=abcdef1234567890abcdef1234567890

# Optional: Paths
# TG_SYNC_DATA_DIR=./data
# TG_SYNC_SESSION_PATH=./session.db

# Optional: Rate limiting (avoid FLOOD_WAIT)
# EXPORT_DELAY_MS=500
# SYNC_DELAY_MS=500

# Optional: Backpressure & watcher
# TG_SYNC_MEDIA_QUEUE_SIZE=1000
# TG_SYNC_WATCHER_CYCLE_SECS=600

# Optional: AI Analysis (OpenAI or Ollama)
# TG_SYNC_AI_API_KEY=sk-...
# TG_SYNC_AI_API_URL=https://api.openai.com/v1/chat/completions
# TG_SYNC_AI_MODEL=gpt-4o-mini

# Optional: Trello (action items from AI analysis)
# TRELLO_KEY=... & TRELLO_TOKEN=... from https://trello.com/app-key
# TRELLO_LIST_ID=... (required for creating cards)
# TRELLO_BOARD_ID=... (optional)

# Optional: External config file
# TG_SYNC_CONFIG=config.toml
```

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `TG_SYNC_API_ID` | **Yes** | — | Telegram API ID |
| `TG_SYNC_API_HASH` | **Yes** | — | Telegram API Hash |
| `TG_SYNC_DATA_DIR` | No | `./data` | Directory for messages.db, media, state.json, reports |
| `TG_SYNC_SESSION_PATH` | No | `./session.db` | Path to persistent MTProto session |
| `SYNC_DELAY_MS` | No | `500` | Milliseconds between message batch requests |
| `EXPORT_DELAY_MS` | No | — | Milliseconds before each GetHistory API call |
| `TG_SYNC_MEDIA_QUEUE_SIZE` | No | `1000` | Bounded channel capacity for media pipeline |
| `TG_SYNC_WATCHER_CYCLE_SECS` | No | `600` | Seconds between watcher sync cycles (daemon mode) |
| `TG_SYNC_AI_API_KEY` | No | — | OpenAI (or compatible) API key; if unset, AI Analysis uses a mock adapter |
| `TG_SYNC_AI_API_URL` | No | OpenAI URL | Chat completions endpoint (e.g. Ollama: `http://localhost:11434/v1/chat/completions`) |
| `TG_SYNC_AI_MODEL` | No | `gpt-4o-mini` | Model name (e.g. Ollama: `llama3.2`, `mistral`) |
| `TRELLO_KEY` | No | — | Trello API key (from [trello.com/app-key](https://trello.com/app-key)) |
| `TRELLO_TOKEN` | No | — | Trello API token |
| `TRELLO_LIST_ID` | No | — | List ID where action-item cards are created (required for Trello) |
| `TRELLO_BOARD_ID` | No | — | Board ID (optional) |

---

## Usage

```bash
# Run in release mode (recommended for production)
cargo run --release

# Or run the compiled binary directly
./target/release/tg-sync
```

On first run, you are prompted to sign in: phone number, verification code (Telegram/SMS), and 2FA password if enabled. Session data is stored in `session.db`, so later runs reuse the session without re-auth.

---

## Output Structure

```
./
├── session.db              # MTProto session (persistent login)
└── data/
    ├── messages.db         # SQLite database (all chats, WAL mode)
    ├── state.json          # Sync checkpoints (last_message_id per chat)
    ├── media/              # Downloaded media files ({chat_id}_{msg_id}.ext)
    │   ├── -1002958729758_23807.jpg
    │   └── ...
    └── reports/            # AI Analysis weekly digests (Markdown)
        ├── analysis_108356540_2026-04.md
        └── ...
```

---

## Tech Stack

| Category | Library | Purpose |
|----------|---------|---------|
| **Runtime** | [tokio](https://tokio.rs/) | Asynchronous runtime (multi-threaded) |
| **Telegram** | [grammers](https://github.com/Lonami/grammers) | MTProto client (user-mode, not Bot API) |
| **Database** | [libsql](https://github.com/tursodatabase/libsql) | SQLite with WAL mode, async queries |
| **Serialization** | [serde](https://serde.rs/) / [serde_json](https://github.com/serde-rs/json) | JSON encoding for media refs and state |
| **Error Handling** | [thiserror](https://github.com/dtolnay/thiserror) / [anyhow](https://github.com/dtolnay/anyhow) | Typed domain errors and context-rich failures |
| **Logging** | [tracing](https://tracing.rs/) | Structured, async-aware logging |
| **Configuration** | [config](https://github.com/mehcode/config-rs) / [dotenv](https://github.com/dotenv-rs/dotenv) | Layered config from env vars and files |
| **TUI** | [inquire](https://github.com/mikaelmello/inquire) / [indicatif](https://github.com/console-rs/indicatif) / [crossterm](https://github.com/crossterm-rs/crossterm) | Interactive prompts, progress bars, terminal UI |
| **AI** | [reqwest](https://github.com/reqwest/reqwest) | HTTP client for OpenAI/Ollama chat completions |
| **Time** | [chrono](https://github.com/chronotope/chrono) | Week grouping and report timestamps |

---

## Contact

**Mukhammedali Berektassuly**

> This project was built with 💜 by a 17-year-old developer from Kazakhstan

- Website: [berektassuly.com](https://berektassuly.com)
- Email: [mukhammedali@berektassuly.com](mailto:mukhammedali@berektassuly.com)
- X/Twitter: [@berektassuly](https://x.com/berektassuly)

---

## License

This project is licensed under the MIT License. See [LICENSE](LICENSE) for details.
