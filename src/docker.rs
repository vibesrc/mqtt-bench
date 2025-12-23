use anyhow::{Context, Result};
use bollard::container::{ListContainersOptions, StatsOptions};
use bollard::Docker;
use chrono::{DateTime, Utc};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

/// Container stats sample
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerSample {
    pub rel_ns: u64,
    pub abs_ts: DateTime<Utc>,
    pub phase: String,

    // CPU percentage calculated from precpu_stats vs cpu_stats (like `docker stats`)
    pub cpu_percent: f64,

    // Raw cumulative values (for reference)
    pub cpu_total_ns: u64,
    pub cpu_system_ns: u64,
    pub cpu_online: u32,

    pub memory_usage_bytes: u64,
    pub memory_limit_bytes: u64,

    pub network_rx_bytes: u64,
    pub network_tx_bytes: u64,
}

/// Docker stats collector configuration
#[derive(Clone)]
pub struct DockerStatsConfig {
    pub container: String,
    pub interval: Duration,
}

/// Docker stats collector
pub struct DockerStatsCollector {
    config: DockerStatsConfig,
    docker: Docker,
    stop: Arc<AtomicBool>,
    samples_tx: mpsc::Sender<ContainerSample>,
    start_time: Instant,
}

impl DockerStatsCollector {
    /// Create a new Docker stats collector
    pub async fn new(
        config: DockerStatsConfig,
        stop: Arc<AtomicBool>,
        samples_tx: mpsc::Sender<ContainerSample>,
    ) -> Result<Self> {
        let docker = Docker::connect_with_socket_defaults()
            .context("Failed to connect to Docker daemon")?;

        // Verify container exists
        docker
            .inspect_container(&config.container, None)
            .await
            .context(format!(
                "Container '{}' not found. Make sure the container is running.",
                config.container
            ))?;

        info!(container = %config.container, "Connected to Docker, monitoring container");

        Ok(Self {
            config,
            docker,
            stop,
            samples_tx,
            start_time: Instant::now(),
        })
    }

    /// Try to create a collector, returning None if Docker is unavailable
    pub async fn try_new(
        config: DockerStatsConfig,
        stop: Arc<AtomicBool>,
        samples_tx: mpsc::Sender<ContainerSample>,
    ) -> Option<Self> {
        match Self::new(config, stop, samples_tx).await {
            Ok(collector) => Some(collector),
            Err(e) => {
                warn!("Docker stats collection unavailable: {}", e);
                None
            }
        }
    }

    /// Reset the start time (called after warmup)
    #[allow(dead_code)]
    pub fn reset_start_time(&mut self) {
        self.start_time = Instant::now();
    }

    /// Run the stats collection loop
    pub async fn run(self, phase_fn: impl Fn(Duration) -> String + Send + 'static) -> Result<()> {
        let mut interval = tokio::time::interval(self.config.interval);

        while !self.stop.load(Ordering::Relaxed) {
            interval.tick().await;

            if self.stop.load(Ordering::Relaxed) {
                break;
            }

            let elapsed = self.start_time.elapsed();
            let phase = phase_fn(elapsed);

            match self.collect_stats(&phase).await {
                Ok(sample) => {
                    if self.samples_tx.send(sample).await.is_err() {
                        // Receiver dropped, stop collecting
                        break;
                    }
                }
                Err(e) => {
                    debug!(error = %e, "Failed to collect container stats");
                }
            }
        }

        Ok(())
    }

    /// Collect a single stats sample using streaming mode to get precpu_stats
    async fn collect_stats(&self, phase: &str) -> Result<ContainerSample> {
        // Use streaming mode - Docker only populates precpu_stats in stream mode
        let options = Some(StatsOptions {
            stream: true,
            one_shot: false,
        });

        let mut stream = self.docker.stats(&self.config.container, options);

        // Skip first sample (has no precpu_stats), take second
        let _first = stream.next().await;
        let stats = stream
            .next()
            .await
            .ok_or_else(|| anyhow::anyhow!("No stats returned"))?
            .context("Failed to get container stats")?;

        // Calculate CPU percentage using precpu_stats (like `docker stats` does)
        let cpu_total_ns = stats.cpu_stats.cpu_usage.total_usage;
        let cpu_system_ns = stats.cpu_stats.system_cpu_usage.unwrap_or(0);
        let cpu_online = stats.cpu_stats.online_cpus.unwrap_or(1) as u32;

        let precpu_total_ns = stats.precpu_stats.cpu_usage.total_usage;
        let precpu_system_ns = stats.precpu_stats.system_cpu_usage.unwrap_or(0);

        let cpu_delta = cpu_total_ns.saturating_sub(precpu_total_ns);
        let system_delta = cpu_system_ns.saturating_sub(precpu_system_ns);

        let cpu_percent = if system_delta > 0 && cpu_delta > 0 {
            (cpu_delta as f64 / system_delta as f64) * (cpu_online as f64) * 100.0
        } else {
            0.0
        };

        // Memory values
        let memory_usage_bytes = stats.memory_stats.usage.unwrap_or(0);
        let memory_limit_bytes = stats.memory_stats.limit.unwrap_or(0);

        // Network values (sum across all interfaces)
        let (network_rx_bytes, network_tx_bytes) = stats
            .networks
            .as_ref()
            .map(|networks| {
                networks.values().fold((0u64, 0u64), |(rx, tx), net| {
                    (rx + net.rx_bytes, tx + net.tx_bytes)
                })
            })
            .unwrap_or((0, 0));

        let rel_ns = self.start_time.elapsed().as_nanos() as u64;

        debug!(
            cpu_percent = format!("{:.1}%", cpu_percent),
            memory_mb = memory_usage_bytes / 1_000_000,
            "Container stats sample"
        );

        let sample = ContainerSample {
            rel_ns,
            abs_ts: Utc::now(),
            phase: phase.to_string(),
            cpu_percent,
            cpu_total_ns,
            cpu_system_ns,
            cpu_online,
            memory_usage_bytes,
            memory_limit_bytes,
            network_rx_bytes,
            network_tx_bytes,
        };

        Ok(sample)
    }
}

/// Calculate CPU percentage from two consecutive samples
#[allow(dead_code)]
pub fn calculate_cpu_percent(prev: &ContainerSample, curr: &ContainerSample) -> f64 {
    let cpu_delta = curr.cpu_total_ns.saturating_sub(prev.cpu_total_ns);
    let system_delta = curr.cpu_system_ns.saturating_sub(prev.cpu_system_ns);

    if system_delta == 0 {
        return 0.0;
    }

    (cpu_delta as f64 / system_delta as f64) * (curr.cpu_online as f64) * 100.0
}

/// Calculate memory percentage
#[allow(dead_code)]
pub fn calculate_memory_percent(sample: &ContainerSample) -> f64 {
    if sample.memory_limit_bytes == 0 {
        return 0.0;
    }

    (sample.memory_usage_bytes as f64 / sample.memory_limit_bytes as f64) * 100.0
}

/// Information about a detected container
#[derive(Debug, Clone)]
pub struct DetectedContainer {
    pub id: String,
    pub name: String,
    pub image: String,
}

/// Common MQTT broker image patterns
const MQTT_BROKER_PATTERNS: &[&str] = &[
    "mosquitto",
    "emqx",
    "hivemq",
    "vernemq",
    "rabbitmq",
    "nanomq",
    "rumqtt",
    "aedes",
    "mosca",
    "moquette",
    "activemq",
    "solace",
    "mqtt",
];

/// Check if an image name looks like an MQTT broker
fn is_mqtt_broker_image(image: &str) -> bool {
    let image_lower = image.to_lowercase();
    MQTT_BROKER_PATTERNS.iter().any(|pattern| image_lower.contains(pattern))
}

/// Try to auto-detect a container running on the specified port
pub async fn detect_container_by_port(port: u16) -> Option<DetectedContainer> {
    let docker = match Docker::connect_with_socket_defaults() {
        Ok(d) => d,
        Err(e) => {
            debug!("Docker not available for auto-detection: {}", e);
            return None;
        }
    };

    // List all running containers
    let options = Some(ListContainersOptions::<String> {
        all: false, // Only running containers
        ..Default::default()
    });

    let containers = match docker.list_containers(options).await {
        Ok(c) => c,
        Err(e) => {
            debug!("Failed to list containers: {}", e);
            return None;
        }
    };

    // First pass: Find container with matching port binding
    for container in &containers {
        if let Some(ports) = &container.ports {
            for port_binding in ports {
                // Check if this container exposes the target port
                if let Some(public_port) = port_binding.public_port {
                    if public_port == port {
                        let id = container.id.clone().unwrap_or_default();
                        let name = container
                            .names
                            .as_ref()
                            .and_then(|n| n.first())
                            .map(|n| n.trim_start_matches('/').to_string())
                            .unwrap_or_else(|| id.chars().take(12).collect());
                        let image = container.image.clone().unwrap_or_default();

                        info!(
                            container_name = %name,
                            container_id = %id.chars().take(12).collect::<String>(),
                            image = %image,
                            port = port,
                            "Auto-detected container on port"
                        );

                        return Some(DetectedContainer { id, name, image });
                    }
                }

                // Also check private port (container's internal port)
                if port_binding.private_port == port {
                    let id = container.id.clone().unwrap_or_default();
                    let name = container
                        .names
                        .as_ref()
                        .and_then(|n| n.first())
                        .map(|n| n.trim_start_matches('/').to_string())
                        .unwrap_or_else(|| id.chars().take(12).collect());
                    let image = container.image.clone().unwrap_or_default();

                    info!(
                        container_name = %name,
                        container_id = %id.chars().take(12).collect::<String>(),
                        image = %image,
                        port = port,
                        "Auto-detected container on port"
                    );

                    return Some(DetectedContainer { id, name, image });
                }
            }
        }
    }

    // Second pass: Check for host-networked MQTT broker containers
    // With host networking, ports aren't exposed through Docker, so we look for
    // containers with host network mode that appear to be MQTT brokers
    for container in &containers {
        let id = container.id.clone().unwrap_or_default();
        let image = container.image.clone().unwrap_or_default();

        // Check if container uses host network mode
        let is_host_network = container
            .host_config
            .as_ref()
            .and_then(|hc| hc.network_mode.as_ref())
            .map(|mode| mode == "host")
            .unwrap_or(false);

        if is_host_network && is_mqtt_broker_image(&image) {
            let name = container
                .names
                .as_ref()
                .and_then(|n| n.first())
                .map(|n| n.trim_start_matches('/').to_string())
                .unwrap_or_else(|| id.chars().take(12).collect());

            info!(
                container_name = %name,
                container_id = %id.chars().take(12).collect::<String>(),
                image = %image,
                network_mode = "host",
                "Auto-detected MQTT broker container (host network)"
            );

            return Some(DetectedContainer { id, name, image });
        }
    }

    debug!("No container found exposing port {}", port);
    None
}
