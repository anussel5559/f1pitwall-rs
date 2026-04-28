# f1pitwall-rs

Live F1 timing in the terminal. Streams real-time data from the [OpenF1 API](https://openf1.org/) (HTTP polling + MQTT) with a TUI powered by [Ratatui](https://ratatui.rs/).

This repo holds the open-source pieces of the pitwall project:

- **`f1core`** — Rust library: OpenF1 ingest, replay, parsing, session state, telemetry, tyre/strategy modeling, optional ONNX-based pit-window predictions. Reusable as a dep for other F1 tooling.
- **`pw`** — Terminal UI built on top of `f1core`.

![demo](demo.gif)

## Features

- **Live timing board** — driver positions, gaps, sector times, lap times, tyre compound and age, pit stops
- **Color-coded sectors** — purple for session best, green for personal best, yellow for slower
- **Race control messages** — flags, penalties, and announcements with timestamps
- **Weather** — air/track temperature, humidity, wind speed, rain indicator
- **Session picker** — browse sessions by year
- **Replay mode** — watch past sessions with seek controls; configurable playback speed (0.5x, 1x, 2x, …)
- **Resume** — saves and resumes replay position between sessions
- **Driver telemetry** — press `t` to view live speed, throttle, brake, and gear charts
- **Track map** — press `m` to see selected drivers on the circuit (requires authentication)
- **Efficient polling** — rate-limited API access with hot/slow endpoint rotation
- **Chunked data buffering** — location and telemetry data pre-fetched in 2-minute chunks, buffered 10 minutes ahead into SQLite to minimize API requests
- **OpenF1 authentication** — optional login for higher rate limits; credentials stored in OS keychain

## Supported session types

- Race
- Sprint
- Qualifying
- Sprint Qualifying
- Practice (FP1, FP2, FP3) — data fetched for strategy baselines

## Install

### Homebrew

```sh
brew tap anussel5559/tap
brew install pw
```

### Pre-built binaries

Download the latest release for your platform from [Releases](https://github.com/anussel5559/f1pitwall-rs/releases).

| Platform              | Binary                                |
|-----------------------|---------------------------------------|
| macOS (Apple Silicon) | `pw-aarch64-apple-darwin.tar.gz`      |
| macOS (Intel)         | `pw-x86_64-apple-darwin.tar.gz`       |
| Linux (x86_64)        | `pw-x86_64-unknown-linux-gnu.tar.gz`  |
| Windows (x86_64)      | `pw-x86_64-pc-windows-msvc.zip`       |

### Build from source

Requires [Rust](https://rustup.rs/) 1.85+ (edition 2024).

```sh
git clone https://github.com/anussel5559/f1pitwall-rs.git
cd f1pitwall-rs
cargo build --release -p pw
# Binary at target/release/pw
```

## Usage

```sh
# Open the session picker
pw

# Jump directly to a session by key
pw --session 9690

# Replay at 2x speed
pw --session 9690 --speed 2.0

# Start fresh (wipe local cache)
pw --fresh
```

### Options

| Flag                  | Description                                         |
|-----------------------|-----------------------------------------------------|
| `-s, --session <KEY>` | Session key to display directly (skip picker)       |
| `--db <PATH>`         | Database file path (default: `f1-pitwall.db`)       |
| `--fresh`             | Delete the database and start fresh                 |
| `--speed <SPEED>`     | Playback speed for replays (default: `1.0`)         |
| `--username <USER>`   | OpenF1 username (or set `PW_USERNAME`)              |
| `--password <PASS>`   | OpenF1 password (or set `PW_PASSWORD`)              |

## Keybindings

### Session picker

| Key             | Action            |
|-----------------|-------------------|
| `j` / `Down`    | Navigate down     |
| `k` / `Up`      | Navigate up       |
| `Enter`         | Select session    |
| `h` / `Left`    | Previous year     |
| `l` / `Right`   | Next year         |
| `a`             | Login to OpenF1   |
| `d`             | Logout            |
| `q` / `Esc`     | Quit              |

### Timing board

| Key              | Action                                            |
|------------------|---------------------------------------------------|
| `j` / `Down`     | Move selection down                               |
| `k` / `Up`       | Move selection up                                 |
| `t`              | Open telemetry for selected driver                |
| `m`              | Toggle track map (authenticated only)             |
| `Space`          | Toggle driver on track map (authenticated only)   |
| `r`              | Toggle race control panel                         |
| `Left`           | Seek back 10s (replay)                            |
| `Right`          | Seek forward 10s (replay)                         |
| `Shift+Left`     | Seek back 60s (replay)                            |
| `Shift+Right`    | Seek forward 60s (replay)                         |
| `q` / `Esc`      | Back to picker                                    |

### Telemetry view

| Key              | Action                  |
|------------------|-------------------------|
| `j` / `Down`     | Next driver             |
| `k` / `Up`       | Previous driver         |
| `t` / `Esc`      | Close telemetry         |
| `Left`           | Seek back 10s (replay)  |
| `Right`          | Seek forward 10s (replay) |
| `Shift+Left`     | Seek back 60s (replay)  |
| `Shift+Right`    | Seek forward 60s (replay) |
| `q`              | Quit                    |

## Authentication

`pw` works without authentication using the free public API. [OpenF1 sponsors](https://openf1.org/) get higher rate limits (60 vs 30 req/min) and faster data updates.

To log in, press `a` from the session picker. Credentials are stored in your OS keychain (macOS Keychain, Windows Credential Manager, or Linux secret-service) and persist across sessions. You can also pass credentials via CLI flags or environment variables:

```sh
# Environment variables (recommended)
export PW_USERNAME=you@example.com
export PW_PASSWORD=yourpassword
pw

# CLI flags
pw --username you@example.com --password yourpassword
```

Tokens expire hourly and are refreshed automatically in the background.

## Using `f1core` in your own crate

```toml
[dependencies]
f1core = { git = "https://github.com/anussel5559/f1pitwall-rs" }
```

Optional `ml` feature enables the bundled ONNX-based pit-window predictor (pulls in `ort` + `ndarray`):

```toml
f1core = { git = "https://github.com/anussel5559/f1pitwall-rs", features = ["ml"] }
```

The trained `pit_window_q{25,50,75}.onnx` quantile models ship with the crate. The training pipeline (data + scripts) is private; if you want to retrain, you'll need your own dataset.

## Architecture

```
crates/
├── f1core/       # Shared library — no UI dependencies
│   ├── api/            # OpenF1 HTTP client + data models
│   ├── auth            # OAuth tokens + OS keychain
│   ├── clock           # Virtual session clock (live + replay)
│   ├── db/             # SQLite persistence (rusqlite)
│   ├── domain/         # Business rules (positions, sectors, DRS, track outlines, degradation, strategy, alerts, ml)
│   ├── mqtt            # Live MQTT streaming (ingests car_data + location)
│   ├── polling         # API fetch orchestration + replay idle loop
│   ├── session_data    # SessionData, DisplayRow, BoardRows types
│   ├── session_types   # SessionType/Endpoint enums + bootstrap logic
│   ├── telemetry       # Car telemetry state + chart-state refresh from SQLite
│   ├── toast           # Notification system
│   └── util            # Time + GMT-offset parsing helpers
└── pw/           # Terminal frontend — ratatui + crossterm
    ├── app             # AppState (wraps f1core types + UI state)
    ├── pages/          # Session, picker, login page controllers
    ├── ui/             # All ratatui rendering (board, telemetry charts, track map)
    └── update          # Self-update via GitHub releases

data/
├── tracks/             # Per-circuit outlines (embedded at compile time via `include_dir!`)
├── models/             # Trained ONNX pit-window models (embedded under `ml` feature)
└── compound_allocations.json   # Tyre compound allocations per race
```

`f1core` contains all business logic with zero UI dependencies. `pw` is the reference consumer.

## Data source

All data comes from the [OpenF1 API](https://openf1.org/), a free and open-source API for Formula 1 live timing data.

## License

MIT — see [LICENSE](./LICENSE).
