# Changelog

## 0.34.2

- Fix every qualifying driver getting stuck on the "PIT" label (`crates/f1core/src/db/queries.rs`)
  - Symptom from a live qualifying session: nearly every driver — including ones currently setting sectors — was rendered with the "PIT" status overlay on the leaderboard. The frontend reads `row.in_pit` and prefers it over every other state, so the bad flag effectively masked all other on-track states
  - Root cause: 0.34.0 introduced a `qual_pit_status` CTE that aggregated **all** of a driver's `pit_stops` rows with `MAX(CASE WHEN ... lane_duration IS NULL ... THEN 1 ELSE 0 END)`. The intent was to mirror the race-side `pit_status` CTE, but the qualifying use case is materially different: drivers traverse the pit lane many times between hot laps, and OpenF1 commonly leaves `lane_duration` NULL on those qualifying traversal rows (it's primarily backfilled for race stops). With NULL `lane_duration` the `CASE` evaluates to 1, and `MAX` then locks `in_pit = 1` for the rest of the session — for every driver who'd ever entered the pit lane
  - Fix: drop the `qual_pit_status` CTE entirely and reuse the existing `closed_in_laps` gate that `is_in_lap` already relies on. `in_pit` is now `1` iff the driver's current lap matches a confirmed in-lap (successor stint exists, or `pit_stops` row at the stint's `lap_end`) **and** that lap has finished by `clock_now`. Once the driver starts an out-lap (next stint), the cil join misses and `in_pit` clears — no more "stuck PIT" from stale NULL-lane_duration traversals. Same evidence threshold as `is_in_lap`, just additionally gated on lap completion so the label transitions IN → PIT at the moment the in-lap timer rolls over rather than at pit-entry
  - Tests: 1 new regression test (`qualifying_in_pit_clears_after_out_lap_with_null_lane_duration`) covering the multi-traversal NULL `lane_duration` case; the existing `qualifying_in_pit_follows_pit_stops_timing` test continues to pass under the new gate

## 0.34.1

- Fix qualifying leaderboard ordering of eliminated drivers (`crates/f1core/src/display.rs`)
  - Symptom: in qualifying sessions past Q1, drivers eliminated in Q2 were displayed in slots P16–P20 while drivers eliminated in Q1 were shown in P11–P15 — the inverse of correct order. A Q2-eliminated driver outranks a Q1-eliminated driver because they advanced through one more cut, so they should occupy P11–P15 (with Q1 eliminations in P16–P20)
  - Root cause: `build_qualifying_rows` partitions eliminated drivers into a separate `elim_rows` block and sorts that block by the `knocked_out` string tag (`"Q1"` / `"Q2"`) before assigning positions sequentially after the active rows. The sort used `a.knocked_out.cmp(&b.knocked_out)`, and lexicographically `"Q1" < "Q2"`, so Q1 eliminations were placed first in the block and received the lower position numbers. The accompanying comment ("Q1 first, then Q2") matched the buggy implementation but not the F1 sporting reality
  - Fix: swap to `b.knocked_out.cmp(&a.knocked_out)` so the most-recent elimination round sorts first within the block. The `.then_with(...)` lap-time tiebreaker is unchanged — within a single segment, faster best lap still ranks higher. No data-shape changes; purely a comparator flip plus an updated comment

## 0.34.0

- Stop misclassifying live flying laps as in-laps (`crates/f1core/src/db/queries.rs`, `crates/f1core/src/db/models.rs`)
  - Symptom from a live FP1 (Miami 2026): LEC was setting personal-best sectors and purple sector times but the on-track status indicator showed "IN" (in-lap) on every fresh lap. Same defect surfaces during any live session — it just hadn't been called out before because most users were on replays
  - Root cause: `is_in_lap` (and `in_pit` in the qualifying/practice query) were computed by checking `laps.lap_number = stints.lap_end`. OpenF1's stints endpoint reports each stint's `lap_end` as "the latest completed lap of that stint" — for an *open* stint (one currently being run live), `lap_end` advances every time the driver crosses the line. So once the stints poll caught up, every fresh lap matched `lap_end` and was flagged as an in-lap. The same broken assumption was filtering laps out of `completed_laps` / `prev_non_outlap`, intermittently dropping live PB laps from best-lap calculations between the laps poll and the next stints poll
  - Fix: introduce a `closed_in_laps` CTE in both `get_qualifying_board_rows` and `get_race_board_rows` that lists `(driver_number, lap_number)` pairs only for stints we have positive evidence have closed — either a successor stint exists for the same driver, or a `pit_stops` row sits at the stint's `lap_end` with `date <= clock_now`. `is_in_lap` is now `1` iff the current lap matches a row in `closed_in_laps`; everything else (live driver mid-stint, no matter how long the stint has been open) gets `is_in_lap = 0`. Same gate is applied to the `completed_laps` and `prev_non_outlap` filters so live PBs are no longer transiently dropped
  - The qualifying/practice `in_pit` flag was rewritten to mirror the race query's `pit_status` CTE — it now follows `pit_stops.date .. date + lane_duration` directly instead of the broken `lap_number = lap_end` heuristic. A driver shows `in_pit = 1` exactly while their latest pit stop's pit-lane window contains `clock_now`
  - `BoardRow` (race) gains a new `is_in_lap: bool` field, populated by the same gate; `crates/pitwall/src/board.rs` previously reimplemented this check locally with the same defect, and will be updated in a follow-up to consume the gated flag directly
  - Live tradeoff: a real in-lap is now only flagged once we have hard evidence the stint closed (next stint reported, or pit-stops row landed). In practice this means the "IN" label appears the moment the pit-stops endpoint records pit-lane entry — typically a few seconds after the car crosses the timing line at pit entry, before the lap rolls over. The cool-down heuristic (`>4s` slack vs PB sector) absorbs the cruise window. We consciously prefer "FLYING/COOL until proven IN" over the previous "IN until proven otherwise"
  - Tests: 6 new unit tests in `crates/f1core/src/db/queries.rs` covering open-stint suppression, successor-closed and pit-stop-closed stints, in-pit timing, and the parallel race-query gate


## 0.33.3

- Bound MQTT zombie-connection retries to ~15 minutes of total silence before giving up (`crates/f1core/src/mqtt.rs`)
  - Symptom from a Practice 1 session: at ~84/90 min into the live broker stream, the OpenF1 broker dropped the TCP connection and started accepting reconnects but publishing nothing. Heartbeat showed `last_event_secs=0 errors=1` indefinitely; toasts spammed `tls handshake eof` once a second. Nothing in the loop was time-bounded — only a user-quit `stop_tx.send(true)` could stop it. The 90 s stall watchdog only warned and re-armed, never broke out
  - Two bugs in the existing code combined to mask broker death: (a) the event-recv branch reset `last_event_at` on **every** event from the poll task, so error events (and broker-side keepalive PingResps every 30 s on a freshly-reconnected zombie connection) continually masqueraded as activity — the stall watchdog read `last_event_secs=28` forever even with zero real data flowing; (b) the watchdog had no escalation path. The post-mortem against OpenF1's REST `/car_data` confirmed the broker outage was purely an MQTT-side problem — REST kept receiving 3 Hz telemetry through scheduled session end and beyond, so disconnecting on broker silence is the right call
  - Fix:
    - `last_event_at` updates only on `Event::Incoming(Packet::Publish(_))` — real broker data. Errors, ConnAck, PingResp etc. no longer reset the timer, so a dead broker can no longer impersonate a live one via keepalive traffic
    - On the heartbeat tick, if `IDLE_RECONNECT_THRESHOLD` (5 min) elapses with no Publish, the inner loop breaks with `StopReason::IdleReconnect` and the outer loop tears down the zombied EventLoop and reconnects from scratch — same disconnect-then-reconnect path that token refresh already uses, just triggered by silence instead of a 50-min interval
    - A new `idle_cycles` counter tracked across reconnects: each idle break increments it, any successful Publish resets it to 0. After `MAX_IDLE_CYCLES` (3) consecutive idle cycles → `StopReason::IdleGiveUp` and the outer loop breaks too. So a transient OpenF1 outage that recovers within ~15 min keeps the loop alive; one that stays dead caps the noise and stops cleanly. Maximum silent time before permanent stop is 3 × 5 min = 15 min, which the user wanted as the upper bound for "broker is not coming back this session"

## 0.33.2

- Fix MQTT live ingestion silently stalling after ~8 minutes (`crates/f1core/src/mqtt.rs`)
  - Root cause: `rumqttc::EventLoop::poll()` was being awaited directly inside `tokio::select!` alongside `flush_tick` (250ms), `token_refresh`, and `stop`. `EventLoop::poll()` is **not cancellation-safe** — every time another `select!` branch fired (thousands of times per minute via `flush_tick`), the in-flight `poll()` was dropped mid-await, corrupting partial-packet read state and the keep-alive timer. After enough cancel cycles the keep-alive PINGREQ stopped going out, the broker silently closed the TCP connection, and the next `poll()` returned errors that `rumqttc`'s automatic reconnect couldn't recover from. The 8-minute symptom was the time it took the cancellation drift to accumulate into a non-recoverable state. A process restart fixed it because it rebuilt the `EventLoop` from scratch
  - Fix: the `EventLoop` now lives in a dedicated `tokio::spawn`'d task that's the only caller of `poll()` — never cancelled mid-await. Events are forwarded to the main loop through a bounded `tokio::mpsc::channel(4096)`, and the main `select!` reads from `event_rx.recv()` (which IS cancellation-safe) instead of `poll()`. On exit, the poll task is `abort()`ed and its `JoinHandle` is awaited; the `EventLoop` is dropped with the task. Channel capacity is sized for ~20s of headroom at peak car_data + location rates so transient SQLite write hiccups don't backpressure the broker into disconnecting
  - Observability: the module previously logged exclusively through `push_toast` (a 5-entry UI ring buffer in `crate::toast`), so `tracing` subscribers saw nothing when the connection died — hence "no log lines" before a restart. Every error/state-change site now emits a `tracing` event alongside the toast: `info!` on connect/disconnect/token-refresh/stop, `warn!` on poll errors and parse failures, `error!` on flush failures and unexpected poll-task exit
  - New 60s heartbeat log with per-topic message counts (`laps`, `position`, `intervals`, `car_data`, `location`, `other`) and `last_event_secs` — lets operators see ingestion rate at a glance and notice silence immediately. New 90s stall watchdog: if no events arrive while live, logs an `error!` + emits a toast once (re-arms after the next event), giving an explicit signal instead of dead silence
- Bump `rumqttc` 0.24 → 0.25, `rustls` 0.22 → 0.23, `rustls-native-certs` 0.7 → 0.8 (`crates/f1core/Cargo.toml`)
  - `rumqttc` 0.25 is a non-breaking bump for our usage (`TlsConfiguration::Rustls(Arc<ClientConfig>)` and `Transport::tls_with_config` keep the same shape); came along with the cancel-safety fix to stay current
  - `rustls` 0.23 requires a `CryptoProvider` to be installed before `ClientConfig::builder()`; new `ensure_crypto_provider()` calls `rustls::crypto::aws_lc_rs::default_provider().install_default()` once per process via `std::sync::Once`
  - `rustls-native-certs` 0.8 returns `CertificateResult` instead of `Result<Vec<Certificate>>`; partial cert-load errors that were previously swallowed are now surfaced as `tracing::warn!`

## 0.33.1

- Drop `time_delta_s` from Pitwall Manager scoring (`crates/f1core/src/domain/pm_score.rs`)
  - The field was meant to be a "time vs ghost call" component, but post-pit pace measures the team's actual stop, not the player's call quality — two players who called different laps for the same driver would get the same value, so it's orthogonal to call quality and adds noise without insight. Without a counterfactual sim, cutting the field is preferable to keeping a misleading metric
  - Removes `ScoreInputs::time_delta_s`, `ScoreWeights::time_per_tenth`, `ScoreBreakdown::time`, and the corresponding term in `score_breakdown` / `ScoreBreakdown::total`. The `time_delta_s` column in the `resolved_calls` table is left in place — it's a separate persistence concern and the consumer can stop populating it independently. Breaking API change for downstream consumers; the only one (private `f1-pitwall` `crates/pitwall/src/pm.rs`) will be updated in a follow-up

## 0.33.0

- Removed the `FetchFrontier` pre-fetch coordinator and the redundant location/telemetry pollers
  - `crates/f1core/src/buffer.rs` deleted; `fmt_ts` / `parse_ts` moved to `crates/f1core/src/util/time.rs` next to `parse_gmt_offset`. The struct's "buffer 10 minutes ahead in 2-minute chunks" model never made sense for live (data ahead of `now` doesn't exist yet) and was unreachable on replay (`bootstrap_session_data` already pre-loads every driver's full-session car_data + location into SQLite). On live the cursor would march into the future on empty API responses; on replay the code was gated out by `skip_api`. Deleted along with `BUFFER_AHEAD_SECS` / `CHUNK_SECS`
  - `crates/f1core/src/location.rs` deleted entirely. `run_location_polling` was a pure REST duplicate of the `v1/location` topic that `run_mqtt_streaming` already writes to SQLite, and it short-circuited to a stop-only `await` for replays. The track map render path in `pages/session.rs` reads `get_latest_locations` from SQLite directly each frame, which is unchanged. `crates/pw/src/pages/input.rs` `LocationTask` and the start/stop wiring in `pages/session.rs` are gone with it
  - `crates/f1core/src/telemetry.rs` `run_telemetry_polling` renamed to `run_telemetry_chart_refresh` and stripped to its only useful job: read the 360s display window from SQLite into `TelemetryState` every 250ms and call `recompute_charts` when the row count changes. The redundant `OpenF1Client::get_car_data` branch (already gated by `skip_api` on replay, redundant with MQTT on live) and its `Toasts` plumbing are gone. Seek-clear behavior preserved via a local `last_seek_gen` instead of `FetchFrontier::check_seek`
  - `crates/pw/src/pages/input.rs` `TelemetryTask::start` no longer takes `client` or `toasts`. `handle_input` and `run_event_loop` shed the now-unused `client` parameter

## 0.32.0

- TUI replay bootstrap rewritten in `pw` for fast, all-driver chunked fetches
  - `crates/pw/src/bootstrap.rs` (new) replaces `polling::bootstrap_session_data` for `pw` only — the f1core function is untouched so the web backend keeps its existing per-driver behavior. Each chunk is a single all-drivers request (no `driver_number` filter, exposed via the new `OpenF1Client::get_car_data_all_drivers` and the existing `get_location` with an empty driver slice). For a 2h race that's ~16 total requests instead of ~176, dropping the bootstrap from ~7 min to under a minute at 24 req/min
  - Chunks are 15 minutes — empirically the largest window OpenF1 will return all-drivers data for without 422'ing (60min hits the cap, 30min triggers ~2-minute slow downloads), and stays comfortably under the 10s reqwest timeout. Both `date>=` (session start) and `date<=` (chunk end) bounds are always present; requests without a lower bound have been observed to walk back through pre-session samples and either time out or 422
  - Per-chunk retry with 1s/2s/4s backoff (`fetch_with_retry`) — turns transient `error decoding response body` and 5xx blips into eventually-consistent loads instead of permanent gaps. car_data and location for each chunk fire in parallel via `tokio::join!`, sharing the OpenF1 client's rate limiter
  - `crates/pw/src/pages/session.rs` waits for `run_polling`'s session-type bootstrap to populate the drivers table before invoking the bootstrap (previously the bootstrap raced ahead and exited via the empty-drivers early return, leaving the session with zero car_data / location accumulating)
- Bootstrap progress overlay
  - `crates/pw/src/bootstrap.rs` exposes `Status` (`Arc<Mutex<Option<Progress>>>`) updated chunk-by-chunk; `pages/session.rs` shares one with both the bootstrap task and `AppState`. `crates/pw/src/ui/mod.rs` `render_bootstrap_status` draws a top-right braille spinner + `Loading replay data N/M` overlay on top of every view (board, telemetry, track map). Clears the moment the bootstrap finishes; never appears on live sessions
- Caller-controlled rate limit on `OpenF1Client`
  - New additive `OpenF1Client::with_rate_limit(credentials, max_requests_per_minute, min_interval)` in `crates/f1core/src/api/mod.rs`. `new()` delegates to it with the existing 28/55 defaults, so the web backend is unchanged. `crates/pw/src/main.rs` `build_client` calls the new method with conservative values (24/500ms unauth, 50/220ms auth) so a noisy bootstrap can't push the unauthenticated public 30 req/min cap with clock drift
- Hide cancelled 2026 grand prix weekends from the picker
  - `crates/pw/src/pages/picker.rs` adds `CANCELLED_MEETING_KEYS = [1282, 1283]` (Bahrain GP 2026, Saudi Arabian GP 2026) and applies the filter in both `filter_future_sessions` and the paused-sessions list at the top of `picker::run`. OpenF1 still serves these meetings, but neither weekend ran. Pre-season Bahrain testing (meeting_key 1304/1305) is unaffected

## 0.31.1

- Extracted from [anussel5559/f1-pitwall](https://github.com/anussel5559/f1-pitwall) as the open-source slice of the project: `f1core` library + `pw` terminal UI. The pitwall web backend + SvelteKit frontend stay private. Repo is MIT, releases ship `pw` binaries via GitHub Releases + Homebrew tap (`anussel5559/tap`)
- Fix: `Confidence` import in `crates/f1core/src/domain/ml.rs` is now `#[cfg(feature = "ml")]`-gated. Without `ml` it was an unused import (clippy `-D warnings` failure); with `ml` enabled it's required by `update_predictions`. Splitting the import on the cfg matches both consumers of f1core (`pw` builds default features, the private web backend builds with `ml`)

## 0.31.0

- TUI (`pw`) replay fixes: bootstrap, faster refresh, board shortcuts
  - Track map wouldn't open in replays without an OpenF1 login: `m` (open map) and `Space` (toggle driver pin) in `crates/pw/src/pages/input.rs` were gated on `state.authenticated`. The auth requirement is a holdover from when the map's only data source was the on-demand `run_location_polling` chunk-fetch — the rate-limited public endpoint couldn't keep a 22-driver session fed. With v0.28.0's per-driver `bootstrap_session_data` already preloading every driver's `location` rows into SQLite for every replay opened in the web app, the gate is wrong for the TUI: replays read straight from DB and never need an authenticated client. New gate is `state.authenticated || !state.clock.is_live`, so live still requires login but replays are open
  - The TUI never spawned `bootstrap_session_data` — only the web backend did — so a replay opened via `pw -s <key>` started life with empty `car_data` / `location` tables and depended entirely on the on-demand chunked fetches in `run_telemetry_polling` / `run_location_polling` to fill them at 3-second cadence. `crates/pw/src/pages/session.rs` now spawns `polling::bootstrap_session_data` immediately after `run_polling` for replay sessions, so the same pre-load that web users get is now what TUI replay users get
  - Now that the bootstrap covers the full replay window, the per-cycle API fetches in those polling tasks are redundant for replays. `crates/f1core/src/location.rs run_location_polling` short-circuits to a stop-only `await` when `!clock.is_live` — the render loop already reads locations from SQLite each frame, so the task has nothing left to do. `crates/f1core/src/telemetry.rs run_telemetry_polling` keeps running on replays (the SQLite → `TelemetryState` read into chart points is what populates the speed/throttle/brake series) but skips the `client.get_car_data` call and tightens the cycle from 3000ms → 250ms. Together with the render-loop input-poll timeout dropping from 500ms → 100ms in `pages/session.rs`, telemetry charts and the track map both refresh ~12× faster on replay seeks
  - The board view had no on-screen help bar — picker and track map both had one but the main session screen left the user to discover `t` / `m` / `r` / `Space` / `←→` / `p` / `q` from documentation only. `crates/pw/src/ui/mod.rs` `draw_race` adds a 1-line constraint at the bottom that renders `render_board_help`: yellow keys + dark-gray labels for nav / telem / map / pin / race ctrl / seek / Shift-seek (60s) / pause / quit. The `m` and `Space` chips dim when disabled (live + unauthenticated) and a "(login for live map)" hint appends in that case

## 0.30.0

- Lap-boundaries query exposes completed-lap sectors
  - `crates/f1core/src/db/queries.rs` `get_driver_lap_starts` SQL widened to also `SELECT duration_sector_1/2/3, lap_duration` per row; return type became a 6-tuple. `crates/pw/src/pages/session.rs` (the only `f1pitwall-rs` consumer) maps the 6-tuple back to its 2-tuple shape so the TUI's chart code is unchanged. The web backend (now in the private repo) consumes the new fields directly to render completed-lap splits in the battle slide-out
- `Battle.history` exposed for downstream visualization
  - `crates/f1core/src/domain/battle.rs` `Battle` struct gained `pub history: Vec<f64>` — the attacker's interval-to-defender samples, oldest → newest. Sourced from the existing per-attacker `interval_history` map already passed into `analyze_battles`, so no new convergence work, no new SQLite reads. Just surfaces what the slope computation was already consuming. `compute_convergence` still slices the most recent `MAX_HISTORY_LAPS` (6) — exposing more history is for visual context, not analysis

## 0.28.0

- Live MQTT ingestion of `car_data` + `location`
  - `crates/f1core/src/mqtt.rs` adds `v1/car_data` and `v1/location` to the existing OpenF1 MQTT topic set. These are the two highest-rate streams (~3-4 Hz × 22 drivers each, combined ~26 KB/sec inbound), so the event loop buffers them in `Vec<CarData>` / `Vec<Location>` and flushes every 250ms inside a single SQLite transaction (`upsert_car_data` / `upsert_location` already accept slices). The low-rate topics (laps, position, intervals, weather, race control, pit, stints) still take the per-message path through `dispatch_message`
- Replay bootstrap of `car_data` + `location`
  - New `polling::bootstrap_session_data` (`crates/f1core/src/polling.rs`) enumerates drivers via the new `Db::get_driver_numbers`, then fetches per-driver `car_data` and `location` for the full session window in parallel (capped at 4 concurrent fetches via `tokio::task::JoinSet`). Per-driver completeness is gated on row count vs. session duration (`Db::car_data_complete` / `Db::location_complete`): require ≥ 70% of `(date_end - date_start) × 3 Hz` rows to consider a driver complete. Threshold is loose on purpose — drivers who joined late or retired early still register as complete and don't trigger redundant re-fetches every replay open. Bootstrap fetches are bounded to `[date_start, date_end]` so we don't pull pre-race formation/grid telemetry that OpenF1 tags to the same `session_key`
- `FrontierSink` trait + on-demand REST fetch path removed from f1core
  - The trait existed only to keep the now-deleted on-demand REST fetch coherent with MQTT writes in the web backend. With bootstrap (replay) and MQTT (live) both writing straight to SQLite, the abstraction is dead weight. Removed from f1core; consumers read straight from SQLite
- `eprintln!` → `tracing` across f1core
  - Added `tracing = "0.1"` as a workspace dep, `tracing` to f1core. Replaced log sites at the 429 backoff in `f1core::api`, replay bootstrap fetch + upsert errors in `polling`. Each call carries structured fields (`session_key`, `driver`, `error = %e`) so log filters work without parsing message strings. `pw` and `bulk_import` keep their `println!` progress output since that's user-facing, not logging

## 0.26.0

- Per-circuit track records + previous winner on `TrackOutline`
  - New optional `qualifying_record` / `race_lap_record` / `previous_winner` objects on `TrackOutline` (`crates/f1core/src/domain/track.rs`). Records carry `{ time_s, driver, team, year }`; winner is `{ year, driver, team }`. All three are `Option<T>`, so circuits can opt in piecemeal. Fields ride the existing `Json(outline)` serialization — no new endpoint
  - All 24 `data/tracks/*.json` files seeded with hand-curated values: pole-lap record, FIA race lap record, and 2025 race winner
  - `scripts/fetch-tracks.mjs` extended to preserve the three new fields across reruns (same pattern as the existing `race_laps` / `length_km` / turn-`name` preservation). Without this, a rerun to pick up a new circuit would silently wipe the hand-curated history
  - New `scripts/update-track-records.mjs` automates post-season updates: walks OpenF1 `/v1/meetings?year=YYYY`, finds the `Race` + `Qualifying` sessions per meeting, pulls their laps and drivers, computes the fastest lap (filtering `is_pit_out_lap` and null sector-1 durations to drop pit-in laps). Race winner sourced from `/v1/session_result` by `position === 1`. Records only overwrite stored values when the current-year time is faster; `previous_winner` is always overwritten. Supports `--circuit KEY` and `--dry-run`. Sprint sessions are deliberately ignored for record-setting (FIA race lap record is Race-only)

## 0.25.0

- Race results query: `Db::get_race_results`
  - New `crates/f1core/src/db/queries.rs` `get_race_results` composes a `RaceResults` response from existing tables: top-3 from `positions` via `ROW_NUMBER() OVER (PARTITION BY driver_number ORDER BY date DESC)` (DESC puts `NULL` grid-seeded rows last so they only surface if no timestamped position exists), joined with `drivers` for acronym/broadcast name/team colour; fastest lap from `MIN(lap_duration)` excluding pit-out laps. New `PodiumEntry` / `FastestLap` / `RaceResults` types in `db/models.rs` with `#[ts(export)]`
- Track outline data gaps
  - `resolve_circuit` (`crates/f1core/src/domain/track.rs`) gained `"yas marina circuit"` as an alias for the `yas_marina` key. OpenF1's 2025 `circuit_short_name` for Abu Dhabi is `"Yas Marina Circuit"`; the prior match arm only covered `"yas marina" / "abu dhabi" / "yas island"`, so Round 24 2025 fell through to `None`
  - Added `data/tracks/imola.json` (MultiViewer-sourced, 766 pts / 19 turns) with hand-curated `race_laps: 63` / `length_km: 4.909`. File had never been seeded — Imola resolved to key `"imola"` but `TRACK_DATA.get()` returned `None`
  - `scripts/fetch-tracks.mjs` now preserves hand-curated `race_laps` + `length_km` across reruns (same mechanism as turn `name` preservation)

## 0.23.0

- MQTT topic set extended with `v1/pit` and `v1/weather`
  - `crates/f1core/src/mqtt.rs` `TOPICS` gains `v1/pit` and `v1/weather` (with matching `dispatch_message` arms). Pit stops + weather now stream over MQTT instead of only being captured by the one-shot bootstrap, which closes a prior gap where neither refreshed for the duration of a live session
- Practice session support in the TUI
  - `crates/pw/src/app/mod.rs`, `ui/board.rs`: `SessionType::Practice` routes through the qualifying board renderer and `lap_display` path (shape-compatible — no lap counter, sector-based layout). Dedicated long-run panel deferred

## 0.21.0

- Per-circuit track data with MultiViewer-sourced geometry
  - Track data pipeline now pulls from MultiViewer (via OpenF1's `circuit_info_url`): `rotation`, reference-lap polyline, and all corners with explicit `(x, y)` + outward-normal `angle` + optional chicane letter (e.g. `13A` / `13B`)
  - Replaced the single `data/track_outlines.json` blob with per-circuit `data/tracks/{key}.json` files; Rust loader embeds them at compile time via `include_dir`. New `scripts/fetch-tracks.mjs` populates/refreshes them and preserves any hand-curated corner names across reruns

## 0.19.0

- Strategy projection engine: pit window predictions
  - New `crates/f1core/src/domain/strategy` module: computes tyre expiry age from fuel-corrected degradation slope, field evidence, compound defaults, cliff detection, and practice baselines. `PitWindow` per driver: estimated laps remaining, window open/close lap range, confidence tier (High/Medium), human-readable reason. Multiple prediction bounds layered conservatively: within-race field evidence (avg completed stint length), degradation threshold extrapolation, cliff age benchmarks, practice baselines, compound-specific defaults
  - Within-race field evidence: once 3+ drivers complete a stint on the same compound, their actual stint lengths become the primary bound for remaining drivers. Dramatically improves mid-race predictions — at 20 clean laps, 85% of predictions are within 5 laps of actual
  - Compound-specific default tyre life caps (SOFT=18, MEDIUM=26, HARD=35) replace the previous universal 40-lap default
  - Low-confidence pit window predictions (< 4 clean laps) are filtered out entirely to avoid noisy early-race predictions
- Fuel-corrected degradation rates
  - Isolates tyre-only degradation by removing the ~0.06s/lap fuel burn effect. Fixed critical sign error in fuel correction formula that was doubling the fuel effect instead of removing it. Fuel-corrected rates now used for compound benchmarks and pit window projections
- ML-based pit window predictions (`ml` feature)
  - Three ONNX models (q25/q50/q75) trained on 974 stints across 48 races predict remaining stint laps with probability ranges. 24-feature input vector: compound, physical compound (C1-C5), tyre age, deg rate, fuel-corrected deg, field evidence, weather, position, gap, slope acceleration, and more. LORO cross-validation: 5.6 lap MAE, 57% within 5 laps, improves as stint progresses (6.0 at 6 laps → 4.6 at 20 laps). ONNX Runtime inference runs every 500ms tick alongside the heuristic engine — microsecond latency per driver. Models embedded in the binary via `include_bytes!()` behind the `ml` Cargo feature flag; gracefully degrades to heuristic-only when disabled
- Compound allocation table
  - Maps HARD/MEDIUM/SOFT to C1-C5 compound numbers per race weekend. Seeded with 2024-2026 data (51 races) from official FIA/Pirelli allocations. `CREATE TABLE IF NOT EXISTS` + seed-on-startup for zero-migration deployments. Source data in `data/compound_allocations.json`, embedded via `include_str!`
- Practice baselines extracted per compound
  - Avg deg rate, avg pace, sample size for long-run stints. Used by the strategy engine as a fallback bound when within-race field evidence is sparse
- New `bulk-import` CLI binary in `f1core`
  - `cargo run --bin bulk-import -- --years 2024,2025` bootstraps all race sessions into SQLite for model training. Reuses existing `bootstrap()` and polling infrastructure with a non-live clock
- Derived degradation features on `StintSummary`: `recent_3lap_avg`, `recent_3lap_delta`, `slope_acceleration`, `max_lap_delta`

## 0.18.0

- Best-sector queries scoped to qualifying segment
  - `get_best_sectors` and `get_driver_best_sectors` threaded through a `since_date` filter and a `clock_now` time gate that matches the `completed_laps` CTE (lap duration elapsed, stint in-lap excluded, pit-out filtered). Purple sectors correctly reset between Q1/Q2/Q3 instead of carrying session-wide minimums. Also fixes a replay-mode discrepancy where best-sector SQL could see sectors from laps whose completion hadn't elapsed yet, leaving consumers unable to locate an owner

## 0.17.0

- Pause/resume for replay sessions
  - New `paused` state on `SessionClock` with `toggle_pause()` that freezes virtual time without losing position
  - TUI: `p` key toggles pause from Board, TrackMap, and Telemetry views; clock label switches to `PAUSED`

## 0.16.1

- Battle stabilization
  - EMA smoothing on interval gaps (α=0.12, ~4s time constant) damps 500ms noise feeding proximity/urgency scoring; resets on large jumps from overtakes. EMA smoothing on interestingness scores (α=0.3) catches closing-rate jumps that gap smoothing can't reach. Ordering hysteresis: battles must outscore an incumbent by 8 points to displace it; analyzer now returns top 8 candidates so the stabilizer has headroom. New `BattleState` held across ticks
- Suppressed position-based alerts (overtakes, contact imminent, tyre cliff) during safety car, VSC, and red flag periods — only rain and safety car alerts still fire under neutralization

## 0.16.0

- Stint and tyre degradation analysis
  - New `f1core::domain::degradation` engine: groups laps by stint, filters outliers (Q1-based reference), computes linear regression deg rate (s/lap) per stint, detects tyre cliffs (rolling-average jump >= 0.5s)
  - `StintSummary` per driver/stint: compound, lap range, deg rate, lap time deltas relative to stint baseline
  - `TyreCliff` detection: compares rolling 3-lap windows, fires when degradation jumps sharply — severity-based alert priority
  - `CompoundBenchmark`: aggregates deg rates across all stints per compound for cross-driver comparison
- Tyre cliff alerts: new `TyreCliff` variant in the alert engine. Fires on rising edge when a cliff is newly detected, with cooldown per driver/stint
- Track status detection: `FINISHED` after `CHEQUERED FLAG` / `SESSION FINISHED` race control messages; chequered flag indicator per driver when they post S3 on the final lap
- Fixed pre-race lap data appearing before formation lap — laps with null `date_start` from OpenF1 API are now excluded from board queries and `current_lap` computation

## 0.15.0

- Alert details now include tyre strategy context
  - Overtake and contact imminent alerts emit compound and age comparison: "Mediums (8 laps) vs Hards (22 laps)". Suppressed when both drivers are on the same compound with similar age (<5 lap difference)

## 0.14.0

- "Watch This" alert engine in `f1core::domain::alert`
  - Rising-edge analysis compares successive board snapshots to detect overtakes, imminent contact, safety car/VSC/red flag, and rain onset. Overtake alerts: position swap detection with consolidated multi-car passes, narrative headlines ("VER takes the lead from HAM"), position-aware priority (P1–P3 hot, P4–P10 warm with tracked battle). Contact imminent alerts: fires when laps-to-contact drops below 3, single best candidate per tick to avoid train spam, auto-resolves with green checkmark and "Overtook" when the pass completes
  - Pit-induced suppression: drivers with `in_pit`, `is_in_lap`, `is_pit_out`, or 3+ position drop in one tick are excluded from overtake and contact alerts. Post-overtake noise prevention: bidirectional cooldowns suppress "closing on" alerts after a position swap, reverse-pair cooldown prevents the overtaken driver from immediately triggering a false attack alert
  - 5-lap cooldown per event key, first 2 laps suppressed entirely (lap 1 chaos). Safety car and rain alerts fire at all laps
- Battle detection migrated to Rust
  - Convergence analysis, pressure scoring, and interestingness ranking now in `f1core::domain::battle`. Interval history queried directly from the database instead of accumulated in-memory across consumer ticks. Eliminates duplicated computation across multiple connected clients; battle state no longer lost on page refresh or tab switch
- Fixed pit out lap styling appearing before the driver actually pits in replay mode — `is_pit_out_lap` now gated on `pit_exit_confirmed`
- Renamed "In DRS range" to "Overtake available" to reflect 2026 regulations

## 0.11.1

- Renamed project from `f1tui` to `f1-pitwall` to reflect the expanded scope beyond just a TUI
- TUI binary renamed from `f1tui` to `pw`
- TUI crate moved from `crates/f1tui` to `crates/pw`
- Data directory renamed from `f1tui` to `f1-pitwall` (DB path, keychain service, MQTT client ID)
- Environment variables renamed from `F1TUI_USERNAME`/`F1TUI_PASSWORD` to `PW_USERNAME`/`PW_PASSWORD`
- Release artifacts renamed from `f1tui-*` to `pw-*`

## 0.11.0

- Architecture cleanup
  - Extracted shared board display logic into `f1core::display` — progressive sector reveal, qualifying elimination/positioning, and best-sectors computation now live in one place instead of being duplicated across consumers
  - Race board SQL refactor: replaced 3 correlated subqueries (pit count, stopped status, pit status) with CTEs for better maintainability
  - Removed dead code: unused `poll_state` / `session_results` tables, stale query functions, unused API endpoints
  - Eliminated duplicated timestamp formatting between `SessionClock::ceiling()` and `buffer::fmt_ts()`

## 0.10.0

- Driver track positioning in the TUI
  - Press `m` to open track map, `Space` in board view to toggle drivers for display. Canvas rendering with Braille markers: track outline in gray, team-colored driver dots with acronym labels. Selected drivers shown with cyan `*` marker in the board view (race and qualifying). Track map gated behind authenticated clients (location data requires higher rate limits)
- Static track outlines bundled for all 24 circuits on the 2024–2026 calendar (real OpenF1 data, ~300 points each)
- Chunked pre-fetch buffering for location and telemetry data (shared `FetchFrontier`)
  - 2-minute chunks fetched into SQLite, buffered up to 10 minutes ahead. 90% runway threshold — refetch only when buffer drops below 9 minutes. Frontier never regresses (protects against background/sync fetch races). Staleness detection resets frontier when more than a chunk behind
- TUI telemetry now writes to SQLite for code sharing
- New `location` table in SQLite schema for caching driver position data

## 0.9.0

- Read-through SQLite cache for car telemetry data — first view fetches from OpenF1, subsequent views are instant
- Non-blocking incremental telemetry polling — background OpenF1 fetches prevent UI freezes during API stalls

## 0.7.0

- Restructured project as a Cargo workspace with two crates: `f1core` (shared library) and `pw` (terminal frontend)
- `f1core` contains all business logic, API client, database, MQTT, polling, and telemetry with zero UI dependencies
- Added `Serialize` derives to f1core model types for JSON serialization
- Added read-only database connection support to f1core
- Removed unused dependencies from the TUI crate (rusqlite, keyring, rumqttc, rustls now only in f1core)

## 0.6.0

- Added GitHub Actions workflow for automated Homebrew releases
- Added MQTT support for real-time live session data streaming
- Qualifying session fine-tuning and improvements
- Fixed self-update binary location

## 0.5.0

- Added OpenF1 authentication for higher rate limits and faster data updates
- Credentials stored securely in OS keychain (macOS Keychain, Windows Credential Manager, Linux secret-service)
- Added qualifying and sprint qualifying session support
- Codebase modularization and cleanup
- Added Homebrew installation support

## 0.4.0

- Added auto-update capability via `--update` flag
- Background version check notifies of new releases in the session picker
- Updated demo recording

## 0.3.0

- Telemetry enhancements: improved chart rendering and data handling
- Added telemetry scrolling and historical review
- Polling adjustments for better data freshness
- Codebase modularization and isolation of concerns

## 0.2.0

- Added live car telemetry view (speed, throttle, brake, gear charts)

## 0.1.0

- Initial release
- Live timing board with driver positions, gaps, intervals, sector times, lap times
- Tyre compound and age tracking, pit stop detection, DRS status
- Color-coded sectors (purple/green/yellow)
- Race control messages panel
- Weather panel
- Session picker with year browsing
- Replay mode with configurable playback speed and seek controls
- Auto-resume of replay position between runs
- SQLite local caching for offline replay
