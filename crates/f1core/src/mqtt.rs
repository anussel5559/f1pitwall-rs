use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use rumqttc::{AsyncClient, Event, MqttOptions, Packet, QoS, TlsConfiguration, Transport};

use crate::api::OpenF1Client;
use crate::api::models::{
    CarData, Interval, Lap, Location, PitStop, Position, RaceControl, Stint, Weather,
};
use crate::db::Db;
use crate::toast::{Toasts, push_toast};

const BROKER_HOST: &str = "mqtt.openf1.org";
const BROKER_PORT: u16 = 8883;

/// Topics to subscribe to for live session data.
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

    loop {
        let token = match get_token(&client).await {
            Some(t) => t,
            None => {
                push_toast(&toasts, "MQTT: no auth token available".into(), true);
                return;
            }
        };

        let (mqtt_client, mut eventloop) = match connect(&token) {
            Ok(pair) => pair,
            Err(e) => {
                push_toast(&toasts, format!("MQTT connect: {e}"), true);
                return;
            }
        };

        if let Err(e) = subscribe(&mqtt_client).await {
            push_toast(&toasts, format!("MQTT subscribe: {e}"), true);
            return;
        }

        let should_stop = run_event_loop(
            session_key,
            &db,
            &persist_high_rate,
            &toasts,
            &mut eventloop,
            &mut token_refresh,
            &mut stop,
        )
        .await;

        // Disconnect cleanly before potential reconnect.
        let _ = mqtt_client.disconnect().await;

        if should_stop {
            break;
        }

        // Token refresh triggered — loop back to reconnect with fresh token.
        push_toast(&toasts, "MQTT: refreshing token...".into(), false);
    }
}

/// Inner event loop. Returns `true` if the caller should stop entirely,
/// `false` if a token refresh was triggered and we should reconnect.
async fn run_event_loop(
    session_key: i64,
    db: &Arc<Mutex<Db>>,
    persist_high_rate: &Arc<AtomicBool>,
    toasts: &Toasts,
    eventloop: &mut rumqttc::EventLoop,
    token_refresh: &mut tokio::time::Interval,
    stop: &mut tokio::sync::watch::Receiver<bool>,
) -> bool {
    let mut car_buf: Vec<CarData> = Vec::with_capacity(256);
    let mut loc_buf: Vec<Location> = Vec::with_capacity(256);
    let mut flush_tick = tokio::time::interval(BATCH_FLUSH_INTERVAL);
    // Skip the immediate-fire first tick.
    flush_tick.tick().await;

    loop {
        tokio::select! {
            event = eventloop.poll() => {
                match event {
                    Ok(Event::Incoming(Packet::Publish(publish))) => {
                        match publish.topic.as_str() {
                            "v1/car_data" => {
                                if persist_high_rate.load(Ordering::Relaxed) {
                                    match serde_json::from_slice::<CarData>(&publish.payload) {
                                        Ok(d) => {
                                            if d.session_key == Some(session_key) {
                                                car_buf.push(d);
                                            }
                                        }
                                        Err(e) => push_toast(
                                            toasts,
                                            format!("MQTT v1/car_data: {e}"),
                                            true,
                                        ),
                                    }
                                }
                            }
                            "v1/location" => {
                                if persist_high_rate.load(Ordering::Relaxed) {
                                    match serde_json::from_slice::<Location>(&publish.payload) {
                                        Ok(d) => {
                                            if d.session_key == Some(session_key) {
                                                loc_buf.push(d);
                                            }
                                        }
                                        Err(e) => push_toast(
                                            toasts,
                                            format!("MQTT v1/location: {e}"),
                                            true,
                                        ),
                                    }
                                }
                            }
                            other => {
                                if let Err(e) = dispatch_message(
                                    session_key,
                                    other,
                                    &publish.payload,
                                    db,
                                ) {
                                    push_toast(toasts, format!("MQTT {other}: {e}"), true);
                                }
                            }
                        }
                    }
                    Ok(Event::Incoming(Packet::ConnAck(_))) => {
                        push_toast(toasts, "MQTT connected".into(), false);
                    }
                    Err(e) => {
                        push_toast(toasts, format!("MQTT: {e}"), true);
                        // rumqttc will attempt automatic reconnection on the next poll().
                    }
                    _ => {}
                }
            }
            _ = flush_tick.tick() => {
                flush_buffers(session_key, db, toasts, &mut car_buf, &mut loc_buf);
            }
            _ = token_refresh.tick() => {
                flush_buffers(session_key, db, toasts, &mut car_buf, &mut loc_buf);
                return false;
            }
            _ = stop.changed() => {
                flush_buffers(session_key, db, toasts, &mut car_buf, &mut loc_buf);
                return true;
            }
        }
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

    let db = db.lock().unwrap();
    if let Err(e) = db.begin() {
        push_toast(toasts, format!("MQTT batch begin: {e}"), true);
        car_buf.clear();
        loc_buf.clear();
        return;
    }
    if !car_buf.is_empty()
        && let Err(e) = db.upsert_car_data(session_key, car_buf)
    {
        push_toast(toasts, format!("MQTT car_data flush: {e}"), true);
    }
    if !loc_buf.is_empty()
        && let Err(e) = db.upsert_location(session_key, loc_buf)
    {
        push_toast(toasts, format!("MQTT location flush: {e}"), true);
    }
    if let Err(e) = db.commit() {
        push_toast(toasts, format!("MQTT batch commit: {e}"), true);
    }

    car_buf.clear();
    loc_buf.clear();
}

async fn get_token(client: &OpenF1Client) -> Option<String> {
    client.auth_manager()?.get_valid_token().await
}

fn connect(token: &str) -> Result<(AsyncClient, rumqttc::EventLoop)> {
    let client_id = format!("f1-pitwall-{}", rand_suffix());
    let mut opts = MqttOptions::new(&client_id, BROKER_HOST, BROKER_PORT);
    opts.set_credentials("f1-pitwall", token);
    opts.set_keep_alive(Duration::from_secs(30));
    opts.set_clean_session(true);

    // TLS via rustls with system root certificates.
    let mut root_store = rustls::RootCertStore::empty();
    for cert in rustls_native_certs::load_native_certs()
        .map_err(|e| anyhow::anyhow!("failed to load native certs: {e}"))?
    {
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
