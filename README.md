# tg-sync

**The pragmatic, incremental Telegram synchronizer built in Rust.**

[![Rust](https://img.shields.io/badge/rust-1.70%2B-orange.svg)](https://www.rust-lang.org/)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![WIP](https://img.shields.io/badge/status-WIP-yellow.svg)](https://github.com)

`tg-sync` is a high-performance CLI tool that acts as a **Telegram Userbot**. It connects via **MTProto** (using [grammers](https://github.com/Lonami/grammers)), fetches your chat list, and performs **incremental backups** of message history and media to local disk. It integrates with **Chatpack** for post-processing of archived logs.

---

## Architecture

### Hexagonal Architecture (Ports & Adapters)

The project is structured around **Hexagonal Architecture** (Ports & Adapters) to **decouple domain logic from the Grammers API and infrastructure**. The core use cases depend only on abstract **ports**; concrete **adapters** implement Telegram (grammers), file I/O, and UI. This allows:

- **Testability**: Domain and use cases can be tested with mock gateways and repositories.
- **Swapability**: Replacing grammers with another Telegram client would require only new adapters; domain and use cases stay unchanged.
- **Clear boundaries**: Business rules live in `domain` and `usecases`; I/O and framework details live in `adapters`.

### Project Structure

```
tg-sync/
├── src/
│   ├── domain/           # Core entities & errors (Chat, Message, MediaReference)
│   ├── ports/            # Inbound (InputPort) & Outbound (TgGateway, RepoPort, StatePort, ProcessorPort)
│   ├── adapters/         # Implementations of ports
│   │   ├── telegram/     # GrammersTgGateway (grammers Client)
│   │   ├── persistence/  # FsRepo, StateJson (filesystem)
│   │   ├── tools/        # ChatpackProcessor (ProcessorPort)
│   │   └── ui/           # TuiInputPort (inquire prompts)
│   ├── usecases/         # SyncService, MediaWorker, AuthService
│   ├── shared/           # Config (env + optional file)
│   ├── lib.rs
│   └── main.rs           # Wiring: create adapters, inject into use cases, run InputPort
├── Cargo.toml
└── .env.example
```

### Sync Engine (Delta Sync)

The **Sync Engine** uses **`min_id` state tracking** to fetch only **new** messages (delta sync):

1. **StatePort** (e.g. `StateJson`) stores the last synced message ID per chat (`last_message_id`).
2. **SyncService** calls `get_messages(chat_id, min_id, limit)` where `min_id = last_message_id`.
3. The **TgGateway** adapter passes `min_id` to Telegram’s `GetHistory`; only messages with `id > min_id` are requested.
4. After messages are saved via **RepoPort**, the state is updated with the new max message ID.

This minimizes API calls and avoids re-downloading already archived messages.

### Async Media Pipeline (Producer–Consumer)

Media downloads run in a **non-blocking** pipeline:

- **Producer**: During sync, **SyncService** pushes `MediaReference` values into an **unbounded MPSC channel** instead of blocking on downloads.
- **Consumer**: **MediaWorker** reads from the channel and downloads files via **TgGateway**, with a **semaphore** (e.g. 3 concurrent downloads) for rate limiting.
- Text sync and media download run **concurrently**; the sync loop is not blocked by slow media.

---

## Features

- **Interactive CLI (TUI)** — Inquire-based prompts to select chats and run sync.
- **Incremental synchronization** — State management per chat (`last_message_id`); only new messages are fetched.
- **Auto-retry on `FLOOD_WAIT`** — Telegram rate limits (RPC 420) are handled by sleeping for the required time and retrying.
- **Parallel media downloading** — Bounded concurrency for media downloads while sync continues.
- **Chatpack integration** — Processor port for post-processing archived data (e.g. log consolidation); adapter can invoke an external Chatpack tool.

---

## Getting Started

### Prerequisites

- **Rust** toolchain (e.g. 1.70+): [rustup](https://rustup.rs/).
- **Telegram API credentials**: Create an application at [my.telegram.org](https://my.telegram.org/apps) to obtain **API ID** and **API Hash**. These are required for MTProto access.

### Configuration

Copy the example env file and set your credentials:

```bash
cp .env.example .env
```

Edit `.env` with your values:

| Variable | Required | Description |
|----------|----------|-------------|
| `TG_SYNC_API_ID` | Yes | Your Telegram API ID (integer). |
| `TG_SYNC_API_HASH` | Yes | Your Telegram API Hash (string). |
| `TG_SYNC_DATA_DIR` | No | Base directory for data and state (default: `./data`). |
| `TG_SYNC_SESSION_PATH` | No | Session file path (optional; see app docs). |
| `TG_SYNC_CONFIG` | No | Optional config file (e.g. `config.toml`) for extra settings. |

Example `.env`:

```env
TG_SYNC_API_ID=12345678
TG_SYNC_API_HASH=your_api_hash_here
# TG_SYNC_DATA_DIR=./data
# TG_SYNC_SESSION_PATH=session.txt
```

---

## Usage

Build and run (release recommended for performance):

```bash
cargo run --release
```

**Login flow** (when the client is not yet authorized):

1. **Phone** — You are prompted for your phone number (e.g. `+1234567890`).
2. **SMS code** — Enter the one-time code sent by Telegram (app or SMS).
3. **2FA (if enabled)** — If the account has two-factor authentication, you are prompted for the cloud password (hint is shown).

After successful login, the TUI lets you **select chats** to sync. Sync runs incrementally and queues media for background download.

---

## Disclaimer

**Important:** This tool operates as a **Userbot** (using your user account via the Telegram client API). Userbot usage may **violate Telegram’s Terms of Service**. Use at your own risk. The authors and contributors are **not responsible** for any account restrictions, bans, or other consequences resulting from the use of this software. Prefer official APIs and bots where possible.

---

## License

Licensed under the **MIT License**. See [LICENSE](LICENSE) for the full text.
