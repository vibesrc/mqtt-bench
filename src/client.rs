use crate::metrics::MetricsCollector;
use anyhow::Result;
use rumqttc::{AsyncClient, Event, EventLoop, MqttOptions, Packet, QoS};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::time::interval;
use tracing::{debug, trace, warn};

/// Convert u8 QoS to rumqttc QoS
pub fn qos_from_u8(qos: u8) -> QoS {
    match qos {
        0 => QoS::AtMostOnce,
        1 => QoS::AtLeastOnce,
        2 => QoS::ExactlyOnce,
        _ => QoS::AtMostOnce,
    }
}

/// Get current timestamp in nanoseconds since Unix epoch
pub fn timestamp_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64
}

/// Embed timestamp in payload (first 8 bytes)
pub fn embed_timestamp(payload: &mut [u8]) {
    let ts = timestamp_ns();
    payload[0..8].copy_from_slice(&ts.to_le_bytes());
}

/// Extract timestamp from payload
pub fn extract_timestamp(payload: &[u8]) -> Option<u64> {
    if payload.len() < 8 {
        return None;
    }
    Some(u64::from_le_bytes(payload[0..8].try_into().ok()?))
}

/// Calculate latency from embedded timestamp
pub fn calculate_latency_ns(payload: &[u8]) -> Option<u64> {
    let embedded = extract_timestamp(payload)?;
    let now = timestamp_ns();
    Some(now.saturating_sub(embedded))
}

/// Connection retry settings
const INITIAL_RETRY_DELAY: Duration = Duration::from_millis(100);
const MAX_RETRY_DELAY: Duration = Duration::from_secs(5);

/// Publisher configuration
#[derive(Clone)]
pub struct PublisherConfig {
    pub client_id: String,
    pub host: String,
    pub port: u16,
    pub topic: String,
    pub qos: QoS,
    pub payload_size: usize,
    pub rate: u32, // messages per second
    pub connect_timeout: Duration,
}

/// MQTT Publisher task
pub struct Publisher {
    config: PublisherConfig,
    metrics: MetricsCollector,
    stop: Arc<AtomicBool>,
}

impl Publisher {
    pub fn new(config: PublisherConfig, metrics: MetricsCollector, stop: Arc<AtomicBool>) -> Self {
        Self {
            config,
            metrics,
            stop,
        }
    }

    pub async fn run(self) -> Result<()> {
        // Pre-allocate payload buffer
        let mut payload = vec![0u8; self.config.payload_size];

        // Calculate interval between messages
        let interval_duration = if self.config.rate > 0 {
            Duration::from_secs_f64(1.0 / self.config.rate as f64)
        } else {
            Duration::from_secs(1)
        };

        let mut is_first_connect = true;

        // Outer reconnection loop
        'reconnect: loop {
            if self.stop.load(Ordering::Relaxed) {
                return Ok(());
            }

            // Connect with retry
            let (client, mut eventloop) = match self.connect(is_first_connect).await {
                Some(conn) => conn,
                None => return Ok(()), // Connection permanently failed or stopped
            };
            is_first_connect = false;

            let mut publish_interval = interval(interval_duration);
            publish_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

            // Combined publish and event loop
            loop {
                if self.stop.load(Ordering::Relaxed) {
                    let _ = client.disconnect().await;
                    return Ok(());
                }

                tokio::select! {
                    // Handle MQTT events
                    event = eventloop.poll() => {
                        match event {
                            Ok(Event::Incoming(Packet::PubAck(_))) => {
                                self.metrics.counters.inc_acked();
                            }
                            Ok(Event::Incoming(Packet::PubComp(_))) => {
                                self.metrics.counters.inc_acked();
                            }
                            Ok(_) => {}
                            Err(e) => {
                                if !self.stop.load(Ordering::Relaxed) {
                                    debug!(
                                        client_id = %self.config.client_id,
                                        error = %e,
                                        "Publisher disconnected, will reconnect"
                                    );
                                    self.metrics.counters.inc_errors();
                                    continue 'reconnect;
                                }
                                return Ok(());
                            }
                        }
                    }
                    // Publish on interval
                    _ = publish_interval.tick() => {
                        // Embed current timestamp
                        embed_timestamp(&mut payload);

                        // Fill rest with random data
                        if self.config.payload_size > 8 {
                            rand::Rng::fill(&mut rand::thread_rng(), &mut payload[8..]);
                        }

                        match client
                            .publish(&self.config.topic, self.config.qos, false, payload.clone())
                            .await
                        {
                            Ok(_) => {
                                self.metrics.counters.inc_sent(self.config.payload_size as u64);
                                trace!(
                                    client_id = %self.config.client_id,
                                    topic = %self.config.topic,
                                    "Published message"
                                );
                            }
                            Err(e) => {
                                self.metrics.counters.inc_errors();
                                debug!(
                                    client_id = %self.config.client_id,
                                    error = %e,
                                    "Failed to publish"
                                );
                                // Channel error likely means disconnected
                                if e.to_string().contains("Channel") {
                                    continue 'reconnect;
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    /// Connect to broker with retry logic
    async fn connect(&self, is_first: bool) -> Option<(AsyncClient, EventLoop)> {
        if is_first {
            self.metrics.counters.inc_connection_attempt();
        } else {
            self.metrics.counters.inc_reconnection();
        }

        let connect_start = Instant::now();
        let mut retry_delay = INITIAL_RETRY_DELAY;
        let mut attempt = 0u32;

        loop {
            if self.stop.load(Ordering::Relaxed) {
                if is_first {
                    self.metrics.counters.inc_connection_failed();
                }
                return None;
            }

            attempt += 1;
            let mut options = MqttOptions::new(
                &self.config.client_id,
                &self.config.host,
                self.config.port,
            );
            options.set_keep_alive(Duration::from_secs(30));
            options.set_clean_session(true);
            options.set_max_packet_size(256 * 1024, 256 * 1024);

            let (client, mut eventloop) = AsyncClient::new(options, 1000);

            match Self::try_connect(&mut eventloop, &self.stop).await {
                Ok(true) => {
                    if is_first {
                        self.metrics.counters.inc_connection_success();
                        debug!(client_id = %self.config.client_id, attempts = attempt, "Publisher connected");
                    } else {
                        debug!(client_id = %self.config.client_id, attempts = attempt, "Publisher reconnected");
                    }
                    return Some((client, eventloop));
                }
                Ok(false) => {
                    if is_first {
                        self.metrics.counters.inc_connection_failed();
                    }
                    warn!(client_id = %self.config.client_id, "Publisher connection rejected by broker");
                    return None;
                }
                Err(e) => {
                    let elapsed = connect_start.elapsed();
                    if elapsed >= self.config.connect_timeout {
                        if is_first {
                            self.metrics.counters.inc_connection_failed();
                            let err_str = e.to_string();
                            if err_str.contains("Too many open files") {
                                warn!(client_id = %self.config.client_id, "Connection failed: Too many open files. Try: ulimit -n 65535");
                            } else {
                                debug!(
                                    client_id = %self.config.client_id,
                                    attempts = attempt,
                                    elapsed = ?elapsed,
                                    "Publisher connection failed after retries"
                                );
                            }
                        }
                        return None;
                    }

                    debug!(
                        client_id = %self.config.client_id,
                        attempt = attempt,
                        error = %e,
                        retry_in = ?retry_delay,
                        "Publisher connection failed, retrying"
                    );
                    tokio::time::sleep(retry_delay).await;
                    retry_delay = (retry_delay * 2).min(MAX_RETRY_DELAY);
                }
            }
        }
    }

    /// Try to establish connection, returns Ok(true) if connected,
    /// Ok(false) if rejected by broker, Err if connection failed
    async fn try_connect(
        eventloop: &mut EventLoop,
        stop: &Arc<AtomicBool>,
    ) -> Result<bool, rumqttc::ConnectionError> {
        loop {
            if stop.load(Ordering::Relaxed) {
                return Err(rumqttc::ConnectionError::Io(std::io::Error::new(
                    std::io::ErrorKind::Interrupted,
                    "stopped",
                )));
            }

            match eventloop.poll().await {
                Ok(Event::Incoming(Packet::ConnAck(ack))) => {
                    return Ok(ack.code == rumqttc::ConnectReturnCode::Success);
                }
                Ok(_) => continue,
                Err(e) => return Err(e),
            }
        }
    }
}

/// Subscriber configuration
#[derive(Clone)]
pub struct SubscriberConfig {
    pub client_id: String,
    pub host: String,
    pub port: u16,
    pub topic_filter: String,
    pub qos: QoS,
    pub connect_timeout: Duration,
}

/// MQTT Subscriber task
pub struct Subscriber {
    config: SubscriberConfig,
    metrics: MetricsCollector,
    stop: Arc<AtomicBool>,
}

impl Subscriber {
    pub fn new(
        config: SubscriberConfig,
        metrics: MetricsCollector,
        stop: Arc<AtomicBool>,
    ) -> Self {
        Self {
            config,
            metrics,
            stop,
        }
    }

    pub async fn run(self) -> Result<()> {
        let mut is_first_connect = true;

        // Outer reconnection loop
        'reconnect: loop {
            if self.stop.load(Ordering::Relaxed) {
                return Ok(());
            }

            // Connect with retry
            let (client, mut eventloop) = match self.connect(is_first_connect).await {
                Some(conn) => conn,
                None => return Ok(()), // Connection permanently failed or stopped
            };
            is_first_connect = false;

            // Subscribe to topics
            if let Err(e) = client
                .subscribe(&self.config.topic_filter, self.config.qos)
                .await
            {
                warn!(
                    client_id = %self.config.client_id,
                    error = %e,
                    "Failed to subscribe"
                );
            }

            // Main event loop - handle messages
            loop {
                if self.stop.load(Ordering::Relaxed) {
                    let _ = client.disconnect().await;
                    return Ok(());
                }

                match eventloop.poll().await {
                    Ok(Event::Incoming(Packet::SubAck(_))) => {
                        debug!(
                            client_id = %self.config.client_id,
                            topic = %self.config.topic_filter,
                            "Subscribed"
                        );
                    }
                    Ok(Event::Incoming(Packet::Publish(publish))) => {
                        let payload = &publish.payload;
                        let payload_len = payload.len() as u64;

                        // Calculate and record latency
                        if let Some(latency_ns) = calculate_latency_ns(payload) {
                            self.metrics.record_latency(latency_ns);
                            trace!(
                                client_id = %self.config.client_id,
                                topic = %publish.topic,
                                latency_us = latency_ns / 1000,
                                "Received message"
                            );
                        }

                        self.metrics.counters.inc_received(payload_len);
                    }
                    Ok(_) => {}
                    Err(e) => {
                        if !self.stop.load(Ordering::Relaxed) {
                            debug!(
                                client_id = %self.config.client_id,
                                error = %e,
                                "Subscriber disconnected, will reconnect"
                            );
                            self.metrics.counters.inc_errors();
                            continue 'reconnect;
                        }
                        return Ok(());
                    }
                }
            }
        }
    }

    /// Connect to broker with retry logic
    async fn connect(&self, is_first: bool) -> Option<(AsyncClient, EventLoop)> {
        if is_first {
            self.metrics.counters.inc_connection_attempt();
        } else {
            self.metrics.counters.inc_reconnection();
        }

        let connect_start = Instant::now();
        let mut retry_delay = INITIAL_RETRY_DELAY;
        let mut attempt = 0u32;

        loop {
            if self.stop.load(Ordering::Relaxed) {
                if is_first {
                    self.metrics.counters.inc_connection_failed();
                }
                return None;
            }

            attempt += 1;
            let mut options = MqttOptions::new(
                &self.config.client_id,
                &self.config.host,
                self.config.port,
            );
            options.set_keep_alive(Duration::from_secs(30));
            options.set_clean_session(true);
            options.set_max_packet_size(256 * 1024, 256 * 1024);

            let (client, mut eventloop) = AsyncClient::new(options, 10000);

            match Self::try_connect(&mut eventloop, &self.stop).await {
                Ok(true) => {
                    if is_first {
                        self.metrics.counters.inc_connection_success();
                        debug!(client_id = %self.config.client_id, attempts = attempt, "Subscriber connected");
                    } else {
                        debug!(client_id = %self.config.client_id, attempts = attempt, "Subscriber reconnected");
                    }
                    return Some((client, eventloop));
                }
                Ok(false) => {
                    if is_first {
                        self.metrics.counters.inc_connection_failed();
                    }
                    warn!(client_id = %self.config.client_id, "Subscriber connection rejected by broker");
                    return None;
                }
                Err(e) => {
                    let elapsed = connect_start.elapsed();
                    if elapsed >= self.config.connect_timeout {
                        if is_first {
                            self.metrics.counters.inc_connection_failed();
                            let err_str = e.to_string();
                            if err_str.contains("Too many open files") {
                                warn!(client_id = %self.config.client_id, "Connection failed: Too many open files. Try: ulimit -n 65535");
                            } else {
                                debug!(
                                    client_id = %self.config.client_id,
                                    attempts = attempt,
                                    elapsed = ?elapsed,
                                    "Subscriber connection failed after retries"
                                );
                            }
                        }
                        return None;
                    }

                    debug!(
                        client_id = %self.config.client_id,
                        attempt = attempt,
                        error = %e,
                        retry_in = ?retry_delay,
                        "Subscriber connection failed, retrying"
                    );
                    tokio::time::sleep(retry_delay).await;
                    retry_delay = (retry_delay * 2).min(MAX_RETRY_DELAY);
                }
            }
        }
    }

    /// Try to establish connection, returns Ok(true) if connected,
    /// Ok(false) if rejected by broker, Err if connection failed
    async fn try_connect(
        eventloop: &mut EventLoop,
        stop: &Arc<AtomicBool>,
    ) -> Result<bool, rumqttc::ConnectionError> {
        loop {
            if stop.load(Ordering::Relaxed) {
                return Err(rumqttc::ConnectionError::Io(std::io::Error::new(
                    std::io::ErrorKind::Interrupted,
                    "stopped",
                )));
            }

            match eventloop.poll().await {
                Ok(Event::Incoming(Packet::ConnAck(ack))) => {
                    return Ok(ack.code == rumqttc::ConnectReturnCode::Success);
                }
                Ok(_) => continue,
                Err(e) => return Err(e),
            }
        }
    }
}

/// Publisher pool manages multiple publisher clients
pub struct PublisherPool {
    _publishers: Vec<tokio::task::JoinHandle<Result<()>>>,
}

impl PublisherPool {
    pub async fn spawn(
        configs: Vec<PublisherConfig>,
        metrics: MetricsCollector,
        stop: Arc<AtomicBool>,
    ) -> Self {
        let mut publishers = Vec::with_capacity(configs.len());

        for config in configs {
            let publisher = Publisher::new(config, metrics.clone(), stop.clone());
            let handle = tokio::spawn(async move { publisher.run().await });
            publishers.push(handle);
        }

        Self { _publishers: publishers }
    }
}

/// Subscriber pool manages multiple subscriber clients
pub struct SubscriberPool {
    _subscribers: Vec<tokio::task::JoinHandle<Result<()>>>,
}

impl SubscriberPool {
    pub async fn spawn(
        configs: Vec<SubscriberConfig>,
        metrics: MetricsCollector,
        stop: Arc<AtomicBool>,
    ) -> Self {
        let mut subscribers = Vec::with_capacity(configs.len());

        for config in configs {
            let subscriber = Subscriber::new(config, metrics.clone(), stop.clone());
            let handle = tokio::spawn(async move { subscriber.run().await });
            subscribers.push(handle);
        }

        Self { _subscribers: subscribers }
    }
}
