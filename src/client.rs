use crate::metrics::MetricsCollector;
use anyhow::Result;
use rumqttc::{AsyncClient, Event, EventLoop, MqttOptions, Packet, QoS};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
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
        let mut options = MqttOptions::new(
            &self.config.client_id,
            &self.config.host,
            self.config.port,
        );
        options.set_keep_alive(Duration::from_secs(30));
        options.set_clean_session(true);
        options.set_max_packet_size(256 * 1024, 256 * 1024);

        let (client, mut eventloop) = AsyncClient::new(options, 1000);

        // Pre-allocate payload buffer
        let mut payload = vec![0u8; self.config.payload_size];

        // Calculate interval between messages
        let interval_duration = if self.config.rate > 0 {
            Duration::from_secs_f64(1.0 / self.config.rate as f64)
        } else {
            Duration::from_secs(1)
        };

        let mut publish_interval = interval(interval_duration);
        publish_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        // Spawn event loop handler
        let stop = self.stop.clone();
        let metrics = self.metrics.clone();
        let client_id = self.config.client_id.clone();
        let eventloop_handle = tokio::spawn(async move {
            Self::handle_events(&mut eventloop, &metrics, &stop, client_id).await
        });

        // Main publish loop
        while !self.stop.load(Ordering::Relaxed) {
            publish_interval.tick().await;

            if self.stop.load(Ordering::Relaxed) {
                break;
            }

            // Embed current timestamp
            embed_timestamp(&mut payload);

            // Fill rest with random data (optional, but more realistic)
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
                }
            }
        }

        // Disconnect gracefully
        let _ = client.disconnect().await;
        eventloop_handle.abort();

        Ok(())
    }

    async fn handle_events(
        eventloop: &mut EventLoop,
        metrics: &MetricsCollector,
        stop: &Arc<AtomicBool>,
        client_id: String,
    ) {
        metrics.counters.inc_connection_attempt();
        let mut connected = false;

        loop {
            if stop.load(Ordering::Relaxed) {
                break;
            }

            match eventloop.poll().await {
                Ok(Event::Incoming(Packet::PubAck(_))) => {
                    metrics.counters.inc_acked();
                }
                Ok(Event::Incoming(Packet::PubComp(_))) => {
                    metrics.counters.inc_acked();
                }
                Ok(Event::Incoming(Packet::ConnAck(ack))) => {
                    if ack.code == rumqttc::ConnectReturnCode::Success {
                        metrics.counters.inc_connection_success();
                        connected = true;
                        debug!(client_id = %client_id, "Publisher connected");
                    } else {
                        metrics.counters.inc_connection_failed();
                        warn!(client_id = %client_id, code = ?ack.code, "Publisher connection rejected");
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    if !stop.load(Ordering::Relaxed) {
                        if !connected {
                            metrics.counters.inc_connection_failed();
                            let err_str = e.to_string();
                            if err_str.contains("Too many open files") {
                                warn!(client_id = %client_id, "Connection failed: Too many open files. Try: ulimit -n 65535");
                            } else {
                                warn!(client_id = %client_id, error = %e, "Publisher connection failed");
                            }
                        } else {
                            debug!(client_id = %client_id, error = %e, "Publisher disconnected");
                        }
                        metrics.counters.inc_errors();
                    }
                    break;
                }
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
        let mut options = MqttOptions::new(
            &self.config.client_id,
            &self.config.host,
            self.config.port,
        );
        options.set_keep_alive(Duration::from_secs(30));
        options.set_clean_session(true);
        options.set_max_packet_size(256 * 1024, 256 * 1024);

        let (client, mut eventloop) = AsyncClient::new(options, 10000);

        // Subscribe after connection
        let subscribe_topic = self.config.topic_filter.clone();
        let subscribe_qos = self.config.qos;
        let subscribe_client = client.clone();

        self.metrics.counters.inc_connection_attempt();
        let mut connected = false;

        // Main event loop
        while !self.stop.load(Ordering::Relaxed) {
            match eventloop.poll().await {
                Ok(Event::Incoming(Packet::ConnAck(ack))) => {
                    if ack.code == rumqttc::ConnectReturnCode::Success {
                        self.metrics.counters.inc_connection_success();
                        connected = true;
                        debug!(
                            client_id = %self.config.client_id,
                            "Subscriber connected"
                        );

                        // Subscribe to topics
                        if let Err(e) = subscribe_client
                            .subscribe(&subscribe_topic, subscribe_qos)
                            .await
                        {
                            warn!(
                                client_id = %self.config.client_id,
                                error = %e,
                                "Failed to subscribe"
                            );
                        }
                    } else {
                        self.metrics.counters.inc_connection_failed();
                        warn!(
                            client_id = %self.config.client_id,
                            code = ?ack.code,
                            "Subscriber connection rejected"
                        );
                    }
                }
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

                    // Calculate and record latency (updates both bucket and HDR histograms)
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
                        if !connected {
                            self.metrics.counters.inc_connection_failed();
                            let err_str = e.to_string();
                            if err_str.contains("Too many open files") {
                                warn!(
                                    client_id = %self.config.client_id,
                                    "Connection failed: Too many open files. Try: ulimit -n 65535"
                                );
                            } else {
                                warn!(
                                    client_id = %self.config.client_id,
                                    error = %e,
                                    "Subscriber connection failed"
                                );
                            }
                        } else {
                            debug!(
                                client_id = %self.config.client_id,
                                error = %e,
                                "Subscriber disconnected"
                            );
                        }
                        self.metrics.counters.inc_errors();
                    }
                    break;
                }
            }
        }

        // Disconnect gracefully
        let _ = client.disconnect().await;

        Ok(())
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
