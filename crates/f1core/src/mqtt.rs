use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use rumqttc::{
    AsyncClient, ConnectionError, Event, MqttOptions, Packet, QoS, TlsConfiguration, Transport,
};

use crate::api::OpenF1Client;
use crate::api::models::{
    CarData, Interval, Lap, Location, PitStop, Position, RaceControl, Stint, Weather,
};
use crate::db::Db;
use crate::toast::{Toasts, push_toast};

const BROKER_HOST: &str = "mqtt.openf1.org";
const BROKER_PORT: u16 = 8883;

const TOPICS: &[&str] = &[
    "v1/laps",
    "v1/position",
    "v1/intervals",
    "v1/stints",
    "v1/race_control",
    "v1/pit",
    "v1/weather",
    "v1/car_data",
    "v1/location",
];

/// How often to refresh the MQTT connection with a new token (50 minutes).
const TOKEN_REFRESH_INTERVAL: Duration = Duration::from_secs(50 * 60);

/// How often to flush buffered high-rate messages (car_data, location) to SQLite.
const BATCH_FLUSH_INTERVAL: Duration = Duration::from_millis(250);

/// How often to log a heartbeat with message rates.
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(60);

/// If no events arrive for this long while live, the connection is presumed dead.
const STALL_THRESHOLD: Duration = Duration::from_secs(90);

/// If no data (Publish) events arrive for this long, the current connection is
/// presumed zombied (the OpenF1 broker has been observed accepting reconnects
/// while silently publishing nothing). Tear down and try a fresh connection.
const IDLE_RECONNECT_THRESHOLD: Duration = Duration::from_secs(5 * 60);

/// Number of consecutive idle reconnect cycles before giving up entirely.
/// Total silent time before permanent stop = MAX_IDLE_CYCLES * IDLE_RECONNECT_THRESHOLD
/// (15 min at the defaults). Any successful Publish resets the counter, so a
/// recovered broker keeps the loop alive indefinitely.
const MAX_IDLE_CYCLES: u32 = 3;

/// Capacity of the channel between the dedicated poll task and the main event loop.
/// Sized for ~20s of headroom at peak car_data + location rates (~200 msg/s).
const EVENT_CHANNEL_CAPACITY: usize = 4096;

/// Run MQTT-based live streaming for a session.
///
/// Subscribes to all relevant topics and upserts incoming messages into the DB.
/// Handles token refresh by disconnecting and reconnecting before the 1-hour expiry.
///
/// `persist_high_rate` gates ingestion of `v1/car_data` and `v1/location` —
/// when `false`, those messages are dropped on receive (no parse, no buffer,
/// no SQLite write). Used to skip practice telemetry when no client is watching.
pub async fn run_mqtt_streaming(
    session_key: i64,
    client: Arc<OpenF1Client>,
    db: Arc<Mutex<Db>>,
    persist_high_rate: Arc<AtomicBool>,
    toasts: Toasts,
    mut stop: tokio::sync::watch::Receiver<bool>,
) {
    let mut token_refresh = tokio::time::interval(TOKEN_REFRESH_INTERVAL);
    // First tick fires immediately — skip it since we just connected.
    token_refresh.tick().await;
    let mut idle_cycles: u32 = 0;

    loop {
        let token = match get_token(&client).await {
            Some(t) => t,
            None => {
                tracing::error!(session_key, "MQTT: no auth token available — aborting");
                push_toast(&toasts, "MQTT: no auth token available".into(), true);
                return;
            }
        };

        let (mqtt_client, eventloop) = match connect(&token) {
            Ok(pair) => pair,
            Err(e) => {
                tracing::error!(session_key, error = %e, "MQTT connect failed");
                push_toast(&toasts, format!("MQTT connect: {e}"), true);
                return;
            }
        };

        if let Err(e) = subscribe(&mqtt_client).await {
            tracing::error!(session_key, error = %e, "MQTT subscribe failed");
            push_toast(&toasts, format!("MQTT subscribe: {e}"), true);
            return;
        }

        tracing::info!(session_key, "MQTT streaming started");

        let reason = run_event_loop(
            session_key,
            &db,
            &persist_high_rate,
            &toasts,
            eventloop,
            &mut token_refresh,
            &mut stop,
            &mut idle_cycles,
        )
        .await;

        // Disconnect cleanly before potential reconnect. The poll task is already
        // aborted by run_event_loop; this just tells the broker we're going.
        let _ = mqtt_client.disconnect().await;

        match reason {
            StopReason::Stop | StopReason::PollerDied => {
                tracing::info!(session_key, ?reason, "MQTT streaming stopped");
                break;
            }
            StopReason::IdleGiveUp => {
                tracing::info!(
                    session_key,
                    idle_cycles,
                    "MQTT streaming stopped after consecutive idle cycles",
                );
                break;
            }
            StopReason::TokenRefresh => {
                tracing::info!(session_key, "MQTT: refreshing token, reconnecting");
                push_toast(&toasts, "MQTT: refreshing token...".into(), false);
            }
            StopReason::IdleReconnect => {
                tracing::info!(
                    session_key,
                    idle_cycles,
                    "MQTT: idle cycle, reconnecting fresh",
                );
            }
        }
    }
}

/// Inner event loop. Returns the reason it exited so the outer loop can decide
/// whether to reconnect or stop.
///
/// This function spawns a dedicated task to drive `EventLoop::poll()` and
/// receives events through an mpsc channel. This is critical: `EventLoop::poll()`
/// is NOT cancellation-safe. Calling it directly inside `tokio::select!` (where
/// other branches can cause it to be dropped mid-await) corrupts internal state
/// — partial packet buffers, the keep-alive timer, in-flight tracking — and
/// leads to silent stalls after a few minutes. mpsc::Receiver::recv IS
/// cancellation-safe, so the timer/stop branches are now safe to fire.
#[allow(clippy::too_many_arguments)]
async fn run_event_loop(
    session_key: i64,
    db: &Arc<Mutex<Db>>,
    persist_high_rate: &Arc<AtomicBool>,
    toasts: &Toasts,
    mut eventloop: rumqttc::EventLoop,
    token_refresh: &mut tokio::time::Interval,
    stop: &mut tokio::sync::watch::Receiver<bool>,
    idle_cycles: &mut u32,
) -> StopReason {
    let mut car_buf: Vec<CarData> = Vec::with_capacity(256);
    let mut loc_buf: Vec<Location> = Vec::with_capacity(256);
    let mut flush_tick = tokio::time::interval(BATCH_FLUSH_INTERVAL);
    flush_tick.tick().await;
    let mut heartbeat_tick = tokio::time::interval(HEARTBEAT_INTERVAL);
    heartbeat_tick.tick().await;

    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel::<PollResult>(EVENT_CHANNEL_CAPACITY);

    // Spawn the dedicated poll task. It owns the EventLoop and is the ONLY
    // place that calls .poll(), so .poll() is never cancelled mid-await.
    let poll_handle = tokio::spawn(async move {
        loop {
            let event = eventloop.poll().await;
            let was_err = event.is_err();
            if event_tx.send(event).await.is_err() {
                // Receiver dropped — main loop is exiting.
                break;
            }
            if was_err {
                // Brief backoff between failed polls so a hard-failed connection
                // doesn't burn a CPU core. rumqttc auto-reconnects on next poll().
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
    });

    let mut counts = MessageCounts::default();
    let mut last_event_at = tokio::time::Instant::now();
    let mut stall_warned = false;

    let stop_reason = loop {
        tokio::select! {
            // Channel recv is cancel-safe — the timer/stop branches firing here
            // does NOT interrupt the eventloop poller.
            maybe_event = event_rx.recv() => {
                let Some(event) = maybe_event else {
                    // Poll task exited (event_tx dropped). Treat as fatal.
                    tracing::error!(session_key, "MQTT poll task exited unexpectedly");
                    push_toast(toasts, "MQTT: poll task died".into(), true);
                    break StopReason::PollerDied;
                };
                // Only treat real Publish events as "session is alive" signals.
                // Errors (TLS EOF on reconnect after session end), ConnAck, and
                // keepalive packets must NOT reset the idle timer — otherwise a
                // dead broker that keeps failing the TLS handshake looks "live"
                // forever and IDLE_STOP_THRESHOLD never trips.
                if matches!(&event, Ok(Event::Incoming(Packet::Publish(_)))) {
                    last_event_at = tokio::time::Instant::now();
                    stall_warned = false;
                    // A real Publish proves the broker is alive — clear the
                    // accumulated dead-cycle count so we don't permanently
                    // give up after a transient outage that recovered.
                    *idle_cycles = 0;
                }
                handle_event(
                    session_key,
                    event,
                    &mut car_buf,
                    &mut loc_buf,
                    &mut counts,
                    persist_high_rate,
                    db,
                    toasts,
                );
            }
            _ = flush_tick.tick() => {
                flush_buffers(session_key, db, toasts, &mut car_buf, &mut loc_buf);
            }
            _ = heartbeat_tick.tick() => {
                let elapsed = last_event_at.elapsed();
                tracing::info!(
                    session_key,
                    laps = counts.laps,
                    position = counts.position,
                    intervals = counts.intervals,
                    car_data = counts.car_data,
                    location = counts.location,
                    other = counts.other,
                    errors = counts.errors,
                    last_event_secs = elapsed.as_secs(),
                    "MQTT heartbeat",
                );
                if elapsed > IDLE_RECONNECT_THRESHOLD {
                    *idle_cycles += 1;
                    if *idle_cycles >= MAX_IDLE_CYCLES {
                        tracing::info!(
                            session_key,
                            idle_cycles = *idle_cycles,
                            last_event_secs = elapsed.as_secs(),
                            "MQTT: {MAX_IDLE_CYCLES} consecutive idle cycles — giving up",
                        );
                        push_toast(
                            toasts,
                            format!(
                                "MQTT stopped: no data for {}m across {} reconnects",
                                elapsed.as_secs() / 60,
                                *idle_cycles
                            ),
                            false,
                        );
                        break StopReason::IdleGiveUp;
                    }
                    tracing::warn!(
                        session_key,
                        idle_cycles = *idle_cycles,
                        last_event_secs = elapsed.as_secs(),
                        "MQTT: idle for {IDLE_RECONNECT_THRESHOLD:?} — tearing down zombie connection, will reconnect",
                    );
                    break StopReason::IdleReconnect;
                }
                if elapsed > STALL_THRESHOLD && !stall_warned {
                    tracing::error!(
                        session_key,
                        last_event_secs = elapsed.as_secs(),
                        "MQTT: no events for {STALL_THRESHOLD:?} — connection may be stalled",
                    );
                    push_toast(
                        toasts,
                        format!("MQTT stalled: {}s without events", elapsed.as_secs()),
                        true,
                    );
                    stall_warned = true;
                }
                counts = MessageCounts::default();
            }
            _ = token_refresh.tick() => {
                tracing::info!(session_key, "MQTT: token refresh interval reached");
                break StopReason::TokenRefresh;
            }
            _ = stop.changed() => {
                tracing::info!(session_key, "MQTT: stop signal received");
                break StopReason::Stop;
            }
        }
    };

    flush_buffers(session_key, db, toasts, &mut car_buf, &mut loc_buf);

    // Abort the poll task — this drops the EventLoop and its underlying
    // connection. Any partially-received packet is discarded, which is fine
    // because we're about to either stop entirely or reconnect with a fresh
    // EventLoop on the next iteration of the outer loop.
    poll_handle.abort();
    let _ = poll_handle.await;

    stop_reason
}

#[derive(Debug)]
enum StopReason {
    /// User-driven shutdown (TUI quit). Final stop.
    Stop,
    /// 50-min token refresh interval reached. Reconnect with a fresh token.
    TokenRefresh,
    /// The dedicated poll task exited unexpectedly. Final stop.
    PollerDied,
    /// No Publish events for IDLE_RECONNECT_THRESHOLD, but we haven't hit
    /// MAX_IDLE_CYCLES yet — tear down this (probably zombied) connection
    /// and reconnect fresh.
    IdleReconnect,
    /// MAX_IDLE_CYCLES consecutive idle cycles — broker has been silent
    /// across multiple reconnects, presumed permanently dead. Final stop.
    IdleGiveUp,
}

#[derive(Default)]
struct MessageCounts {
    laps: u64,
    position: u64,
    intervals: u64,
    car_data: u64,
    location: u64,
    other: u64,
    errors: u64,
}

type PollResult = std::result::Result<Event, ConnectionError>;

#[allow(clippy::too_many_arguments)]
fn handle_event(
    session_key: i64,
    event: PollResult,
    car_buf: &mut Vec<CarData>,
    loc_buf: &mut Vec<Location>,
    counts: &mut MessageCounts,
    persist_high_rate: &Arc<AtomicBool>,
    db: &Arc<Mutex<Db>>,
    toasts: &Toasts,
) {
    match event {
        Ok(Event::Incoming(Packet::Publish(publish))) => match publish.topic.as_str() {
            "v1/car_data" => {
                counts.car_data += 1;
                if persist_high_rate.load(Ordering::Relaxed) {
                    match serde_json::from_slice::<CarData>(&publish.payload) {
                        Ok(d) => {
                            if d.session_key == Some(session_key) {
                                car_buf.push(d);
                            }
                        }
                        Err(e) => {
                            counts.errors += 1;
                            tracing::warn!(error = %e, "MQTT v1/car_data parse failed");
                            push_toast(toasts, format!("MQTT v1/car_data: {e}"), true);
                        }
                    }
                }
            }
            "v1/location" => {
                counts.location += 1;
                if persist_high_rate.load(Ordering::Relaxed) {
                    match serde_json::from_slice::<Location>(&publish.payload) {
                        Ok(d) => {
                            if d.session_key == Some(session_key) {
                                loc_buf.push(d);
                            }
                        }
                        Err(e) => {
                            counts.errors += 1;
                            tracing::warn!(error = %e, "MQTT v1/location parse failed");
                            push_toast(toasts, format!("MQTT v1/location: {e}"), true);
                        }
                    }
                }
            }
            other => {
                match other {
                    "v1/laps" => counts.laps += 1,
                    "v1/position" => counts.position += 1,
                    "v1/intervals" => counts.intervals += 1,
                    _ => counts.other += 1,
                }
                if let Err(e) = dispatch_message(session_key, other, &publish.payload, db) {
                    counts.errors += 1;
                    tracing::warn!(topic = other, error = %e, "MQTT dispatch failed");
                    push_toast(toasts, format!("MQTT {other}: {e}"), true);
                }
            }
        },
        Ok(Event::Incoming(Packet::ConnAck(_))) => {
            tracing::info!(session_key, "MQTT ConnAck received");
            push_toast(toasts, "MQTT connected".into(), false);
        }
        Ok(Event::Incoming(Packet::Disconnect)) => {
            tracing::warn!(session_key, "MQTT broker sent Disconnect");
        }
        Err(e) => {
            counts.errors += 1;
            tracing::warn!(session_key, error = %e, "MQTT poll error (will reconnect)");
            push_toast(toasts, format!("MQTT: {e}"), true);
        }
        _ => {}
    }
}

/// Flush buffered car_data + location to SQLite in a single transaction.
fn flush_buffers(
    session_key: i64,
    db: &Arc<Mutex<Db>>,
    toasts: &Toasts,
    car_buf: &mut Vec<CarData>,
    loc_buf: &mut Vec<Location>,
) {
    if car_buf.is_empty() && loc_buf.is_empty() {
        return;
    }

    let car_n = car_buf.len();
    let loc_n = loc_buf.len();
    let db = db.lock().unwrap();
    if let Err(e) = db.begin() {
        tracing::error!(session_key, error = %e, "MQTT batch begin failed");
        push_toast(toasts, format!("MQTT batch begin: {e}"), true);
        car_buf.clear();
        loc_buf.clear();
        return;
    }
    if !car_buf.is_empty()
        && let Err(e) = db.upsert_car_data(session_key, car_buf)
    {
        tracing::error!(session_key, n = car_n, error = %e, "MQTT car_data flush failed");
        push_toast(toasts, format!("MQTT car_data flush: {e}"), true);
    }
    if !loc_buf.is_empty()
        && let Err(e) = db.upsert_location(session_key, loc_buf)
    {
        tracing::error!(session_key, n = loc_n, error = %e, "MQTT location flush failed");
        push_toast(toasts, format!("MQTT location flush: {e}"), true);
    }
    if let Err(e) = db.commit() {
        tracing::error!(session_key, error = %e, "MQTT batch commit failed");
        push_toast(toasts, format!("MQTT batch commit: {e}"), true);
    }

    car_buf.clear();
    loc_buf.clear();
}

async fn get_token(client: &OpenF1Client) -> Option<String> {
    client.auth_manager()?.get_valid_token().await
}

/// Ensures a default rustls CryptoProvider is installed exactly once per process.
/// rustls 0.23 panics at runtime if no provider is registered when ClientConfig
/// is built without an explicit one.
static CRYPTO_PROVIDER_INIT: std::sync::Once = std::sync::Once::new();

fn ensure_crypto_provider() {
    CRYPTO_PROVIDER_INIT.call_once(|| {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    });
}

fn connect(token: &str) -> Result<(AsyncClient, rumqttc::EventLoop)> {
    ensure_crypto_provider();

    let client_id = format!("f1-pitwall-{}", rand_suffix());
    let mut opts = MqttOptions::new(&client_id, BROKER_HOST, BROKER_PORT);
    opts.set_credentials("f1-pitwall", token);
    opts.set_keep_alive(Duration::from_secs(30));
    opts.set_clean_session(true);

    // TLS via rustls with system root certificates.
    let mut root_store = rustls::RootCertStore::empty();
    let cert_result = rustls_native_certs::load_native_certs();
    for err in &cert_result.errors {
        tracing::warn!(error = %err, "rustls-native-certs: error loading some certs");
    }
    if cert_result.certs.is_empty() {
        return Err(anyhow::anyhow!("no native root certificates loaded"));
    }
    for cert in cert_result.certs {
        let _ = root_store.add(cert);
    }
    let tls_config = rustls::ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();
    let transport = Transport::tls_with_config(TlsConfiguration::Rustls(Arc::new(tls_config)));
    opts.set_transport(transport);

    let (client, eventloop) = AsyncClient::new(opts, 256);
    Ok((client, eventloop))
}

async fn subscribe(client: &AsyncClient) -> Result<()> {
    for topic in TOPICS {
        client.subscribe(*topic, QoS::AtLeastOnce).await?;
    }
    Ok(())
}

fn dispatch_message(
    session_key: i64,
    topic: &str,
    payload: &[u8],
    db: &Arc<Mutex<Db>>,
) -> Result<()> {
    match topic {
        "v1/laps" => {
            let lap: Lap = serde_json::from_slice(payload)?;
            if lap.session_key == Some(session_key) {
                db.lock().unwrap().upsert_lap(session_key, &lap)?;
            }
        }
        "v1/position" => {
            let pos: Position = serde_json::from_slice(payload)?;
            if pos.session_key == Some(session_key) {
                db.lock().unwrap().upsert_position(session_key, &pos)?;
            }
        }
        "v1/intervals" => {
            let interval: Interval = serde_json::from_slice(payload)?;
            if interval.session_key == Some(session_key) {
                db.lock().unwrap().upsert_interval(session_key, &interval)?;
            }
        }
        "v1/stints" => {
            let stint: Stint = serde_json::from_slice(payload)?;
            if stint.session_key == Some(session_key) {
                db.lock().unwrap().upsert_stint(session_key, &stint)?;
            }
        }
        "v1/race_control" => {
            let rc: RaceControl = serde_json::from_slice(payload)?;
            if rc.session_key == Some(session_key) {
                db.lock().unwrap().upsert_race_control(session_key, &rc)?;
            }
        }
        "v1/pit" => {
            let pit: PitStop = serde_json::from_slice(payload)?;
            if pit.session_key == Some(session_key) {
                db.lock().unwrap().upsert_pit_stop(session_key, &pit)?;
            }
        }
        "v1/weather" => {
            let w: Weather = serde_json::from_slice(payload)?;
            if w.session_key == Some(session_key) {
                db.lock().unwrap().upsert_weather(session_key, &w)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn rand_suffix() -> u32 {
    use std::time::SystemTime;
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos()
}
