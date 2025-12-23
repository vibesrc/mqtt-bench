use crate::client::{qos_from_u8, PublisherPool, SubscriberPool};
use crate::db::{Database, FinalResults, RunRecord, ScenarioRecord};
use crate::docker::{detect_container_by_port, ContainerSample, DockerStatsCollector, DockerStatsConfig};
use crate::metrics::MetricsCollector;
use crate::scenarios::{create_scenario, ScenarioParams};
use crate::ScenarioType;
use anyhow::Result;
use chrono::Utc;
use indicatif::{ProgressBar, ProgressStyle};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::{interval, sleep, Instant};
use tracing::{debug, info, warn};
use uuid::Uuid;

/// Benchmark configuration
pub struct BenchmarkConfig {
    pub scenario: ScenarioType,
    pub host: String,
    pub port: u16,
    pub qos: u8,
    pub publishers: Option<u32>,
    pub subscribers: Option<u32>,
    pub topics: Option<u32>,
    pub rate: u32,
    pub payload_size: u32,
    pub warmup: Duration,
    pub duration: Duration,
    pub container: Option<String>,
    pub broker_name: Option<String>,
    pub broker_version: Option<String>,
    pub checkpoint_interval: Duration,
    pub stats_interval: Duration,
    pub timeout: Duration,
    pub notes: Option<String>,
}

/// Benchmark orchestrator
pub struct Orchestrator {
    db: Database,
    config: BenchmarkConfig,
}

impl Orchestrator {
    pub fn new(db: Database, config: BenchmarkConfig) -> Self {
        Self { db, config }
    }

    pub async fn run(self) -> Result<()> {
        let run_id = Uuid::new_v4();
        let scenario_id = Uuid::new_v4();
        let started_at = Utc::now();

        info!(
            run_id = %run_id,
            scenario = %self.config.scenario,
            qos = self.config.qos,
            host = %self.config.host,
            port = self.config.port,
            "Starting benchmark"
        );

        // Get host info
        let hostname = hostname::get()
            .ok()
            .and_then(|h| h.into_string().ok());
        let cpus = num_cpus::get() as i32;

        // Detect container first (before creating run record)
        let container_info = if let Some(container) = &self.config.container {
            Some((container.clone(), container.clone()))
        } else {
            // Try to auto-detect container by port
            match detect_container_by_port(self.config.port).await {
                Some(detected) => {
                    info!(
                        "Auto-detected container '{}' (image: {}) on port {}",
                        detected.name, detected.image, self.config.port
                    );
                    Some((detected.id, detected.name))
                }
                None => {
                    debug!("No container detected on port {}, skipping resource monitoring", self.config.port);
                    None
                }
            }
        };

        // Create run record with container info
        let run_record = RunRecord {
            run_id,
            started_at,
            broker_host: self.config.host.clone(),
            broker_port: self.config.port,
            broker_name: self.config.broker_name.clone(),
            broker_version: self.config.broker_version.clone(),
            container_id: container_info.as_ref().map(|(id, _)| id.clone()),
            container_name: container_info.as_ref().map(|(_, name)| name.clone()),
            hostname,
            cpus: Some(cpus),
            memory_bytes: None,
            git_commit: None,
            notes: self.config.notes.clone(),
        };
        self.db.insert_run(&run_record)?;

        // Create scenario parameters with defaults
        let params = ScenarioParams::new(
            &self.config.scenario,
            self.config.publishers,
            self.config.subscribers,
            self.config.topics,
        );

        // Create scenario record
        let scenario_record = ScenarioRecord {
            scenario_id,
            run_id,
            started_at,
            scenario_name: self.config.scenario.to_string(),
            qos: self.config.qos as i32,
            publishers: params.publishers as i32,
            subscribers: params.subscribers as i32,
            topics: params.topics as i32,
            msg_rate: self.config.rate as i32,
            payload_size: self.config.payload_size as i32,
            warmup_ns: self.config.warmup.as_nanos() as i64,
            duration_ns: self.config.duration.as_nanos() as i64,
        };
        self.db.insert_scenario(&scenario_record)?;

        // Create scenario instance
        let scenario = create_scenario(&self.config.scenario, params.clone());
        let qos = qos_from_u8(self.config.qos);

        // Initialize metrics collector
        let metrics = MetricsCollector::new();

        // Create stop signal
        let stop = Arc::new(AtomicBool::new(false));

        // Setup Docker stats collection using already-detected container info
        let (docker_tx, mut docker_rx) = mpsc::channel::<ContainerSample>(1000);

        let docker_handle = if let Some((container_id, container_name)) = container_info {
            let docker_config = DockerStatsConfig {
                container: container_id,
                interval: self.config.stats_interval,
            };

            if let Some(collector) = DockerStatsCollector::try_new(
                docker_config,
                stop.clone(),
                docker_tx,
            ).await {
                let warmup_duration = self.config.warmup;
                let measure_duration = self.config.duration;

                info!("Monitoring container '{}' for resource usage", container_name);

                Some(tokio::spawn(async move {
                    collector.run(move |elapsed| {
                        if elapsed < warmup_duration {
                            "warmup".to_string()
                        } else if elapsed < warmup_duration + measure_duration {
                            "measurement".to_string()
                        } else {
                            "cooldown".to_string()
                        }
                    }).await
                }))
            } else {
                None
            }
        } else {
            None
        };

        // Spawn Docker sample receiver - collect samples for summary
        let container_samples: Arc<Mutex<Vec<ContainerSample>>> = Arc::new(Mutex::new(Vec::new()));
        let container_samples_clone = container_samples.clone();
        let docker_receiver = tokio::spawn(async move {
            while let Some(sample) = docker_rx.recv().await {
                debug!(
                    memory_mb = %(sample.memory_usage_bytes / 1_000_000),
                    "Container stats"
                );
                container_samples_clone.lock().unwrap().push(sample);
            }
        });

        // Get client configs
        let pub_configs = scenario.publisher_configs(
            &self.config.host,
            self.config.port,
            qos,
            self.config.rate,
            self.config.payload_size as usize,
        );
        let sub_configs = scenario.subscriber_configs(&self.config.host, self.config.port, qos);

        let num_publishers = pub_configs.len();
        let num_subscribers = sub_configs.len();

        let total_clients = num_publishers + num_subscribers;
        info!(
            publishers = num_publishers,
            subscribers = num_subscribers,
            topics = params.topics,
            total_clients = total_clients,
            "Spawning clients"
        );

        // Spawn subscribers first (so they're ready to receive)
        let _subscriber_pool = SubscriberPool::spawn(sub_configs, metrics.clone(), stop.clone()).await;

        // Spawn publishers
        let _publisher_pool = PublisherPool::spawn(pub_configs, metrics.clone(), stop.clone()).await;

        // Wait for all connections to complete (success or failure)
        let connect_timeout = Duration::from_secs(30);
        let connect_start = Instant::now();
        let connect_pb = ProgressBar::new(total_clients as u64);
        connect_pb.set_style(
            ProgressStyle::default_bar()
                .template("{spinner:.blue} [{elapsed_precise}] {bar:40.blue/black} {pos}/{len} {msg}")
                .unwrap()
                .progress_chars("##-"),
        );
        connect_pb.set_message("connecting...");

        loop {
            let succeeded = metrics.counters.connections_succeeded.load(Ordering::Acquire);
            let failed = metrics.counters.connections_failed.load(Ordering::Acquire);
            let completed = succeeded + failed;

            connect_pb.set_position(completed);
            connect_pb.set_message(format!("connecting... ({} ok, {} failed)", succeeded, failed));

            if completed >= total_clients as u64 {
                break;
            }

            if connect_start.elapsed() > connect_timeout {
                warn!(
                    completed = completed,
                    expected = total_clients,
                    "Connection timeout - proceeding with {} of {} clients",
                    succeeded,
                    total_clients
                );
                break;
            }

            sleep(Duration::from_millis(100)).await;
        }

        let final_succeeded = metrics.counters.connections_succeeded.load(Ordering::Acquire);
        let final_failed = metrics.counters.connections_failed.load(Ordering::Acquire);

        if final_failed > 0 {
            connect_pb.finish_with_message(format!(
                "connected: {} ok, {} failed",
                final_succeeded, final_failed
            ));
        } else {
            connect_pb.finish_with_message(format!("all {} clients connected", final_succeeded));
        }

        // Brief pause to let subscriptions complete
        sleep(Duration::from_millis(500)).await;

        // Create progress bar
        let total_duration = self.config.warmup + self.config.duration;
        let pb = ProgressBar::new(total_duration.as_secs());
        pb.set_style(
            ProgressStyle::default_bar()
                .template("{spinner:.green} [{elapsed_precise}] {bar:40.cyan/blue} {pos}/{len}s {msg}")
                .unwrap()
                .progress_chars("##-"),
        );

        // Warmup phase
        info!(duration = ?self.config.warmup, "Starting warmup phase");
        pb.set_message("warmup");

        let warmup_start = Instant::now();
        while warmup_start.elapsed() < self.config.warmup {
            sleep(Duration::from_secs(1)).await;
            pb.inc(1);

            let stats = metrics.throughput_stats();
            pb.set_message(format!(
                "warmup | {:.0} msg/s recv",
                stats.receive_rate
            ));
        }

        // Reset metrics after warmup
        info!("Warmup complete, resetting metrics for measurement phase");
        metrics.reset();
        metrics.reset_start_time();

        // Measurement phase
        info!(duration = ?self.config.duration, "Starting measurement phase");

        let checkpoint_interval = self.config.checkpoint_interval;
        let mut checkpoint_timer = interval(checkpoint_interval);
        let measurement_start = Instant::now();

        loop {
            tokio::select! {
                _ = checkpoint_timer.tick() => {
                    let elapsed = measurement_start.elapsed();

                    if elapsed >= self.config.duration {
                        break;
                    }

                    // Record checkpoint
                    let rel_ns = elapsed.as_nanos() as u64;
                    let abs_ts = Utc::now();

                    // Save histogram bucket snapshot
                    let bucket_snapshot = metrics.bucket_snapshot();
                    self.db.insert_histogram(scenario_id, rel_ns, abs_ts, &bucket_snapshot)?;

                    // Save exact percentiles
                    let exact = metrics.exact_percentiles();
                    self.db.insert_exact_percentiles(scenario_id, rel_ns, abs_ts, &exact)?;

                    // Save counters
                    let counter_snapshot = metrics.counters.snapshot();
                    self.db.insert_counters(scenario_id, rel_ns, abs_ts, &counter_snapshot)?;

                    // Update progress
                    let stats = metrics.throughput_stats();
                    let p99_ms = metrics.current_p99_ms();

                    pb.set_position((self.config.warmup.as_secs() + elapsed.as_secs()).min(total_duration.as_secs()));
                    pb.set_message(format!(
                        "{} @ QoS {} | {:.0} msg/s | P99: {:.1}ms",
                        self.config.scenario,
                        self.config.qos,
                        stats.receive_rate,
                        p99_ms
                    ));
                }
            }
        }

        pb.finish_with_message("measurement complete");

        // Stop publishers first (stop sending new messages)
        info!("Measurement complete, stopping publishers");
        stop.store(true, Ordering::SeqCst);

        // Flush phase - wait for in-flight messages to settle
        info!(
            timeout = ?self.config.timeout,
            "Starting flush phase, waiting for in-flight messages"
        );

        let flush_start = Instant::now();
        let flush_pb = ProgressBar::new(self.config.timeout.as_secs());
        flush_pb.set_style(
            ProgressStyle::default_bar()
                .template("{spinner:.yellow} [{elapsed_precise}] {bar:40.yellow/black} {pos}/{len}s {msg}")
                .unwrap()
                .progress_chars("##-"),
        );
        flush_pb.set_message("flush - waiting for stragglers");

        let pre_flush_received = metrics.counters.messages_received.load(Ordering::Acquire);
        let mut last_received = pre_flush_received;
        let mut idle_seconds = 0u32;

        // Wait for timeout duration, checking for new messages
        while flush_start.elapsed() < self.config.timeout {
            let remaining = self.config.timeout.saturating_sub(flush_start.elapsed());
            let wait_time = remaining.min(Duration::from_secs(1));

            sleep(wait_time).await;

            let current_received = metrics.counters.messages_received.load(Ordering::Acquire);
            let new_messages = current_received - pre_flush_received;

            flush_pb.set_position(flush_start.elapsed().as_secs().min(self.config.timeout.as_secs()));
            flush_pb.set_message(format!(
                "flush - {} stragglers received",
                new_messages
            ));

            // Check if messages stopped arriving (early exit heuristic)
            if current_received == last_received {
                idle_seconds += 1;
                // If no new messages for 3 seconds and we've waited at least 2 seconds total, exit early
                if idle_seconds >= 3 && flush_start.elapsed() >= Duration::from_secs(2) {
                    debug!("No new messages for {}s, exiting flush early", idle_seconds);
                    break;
                }
            } else {
                idle_seconds = 0;
            }
            last_received = current_received;
        }

        let post_flush_received = metrics.counters.messages_received.load(Ordering::Acquire);
        let stragglers = post_flush_received - pre_flush_received;

        // Calculate timed-out messages (sent but never received after flush)
        let total_sent = metrics.counters.messages_sent.load(Ordering::Acquire);
        let total_received = metrics.counters.messages_received.load(Ordering::Acquire);

        // For fan-out scenario, expected = sent * subscribers, for others expected = sent
        // We'll calculate timed_out based on expected vs received in the final stats
        let timed_out = total_sent.saturating_sub(total_received);
        metrics.counters.inc_timed_out(timed_out);

        flush_pb.finish_with_message(format!(
            "flush complete - {} stragglers, {} timed out",
            stragglers, timed_out
        ));

        info!(
            stragglers = stragglers,
            timed_out = timed_out,
            "Flush phase complete"
        );

        // Get final metrics
        let final_stats = metrics.throughput_stats();
        let final_percentiles = metrics.exact_percentiles();
        let duration_secs = self.config.duration.as_secs_f64();

        // Calculate expected messages for delivery rate
        let expected = scenario.expected_messages(self.config.rate, self.config.duration.as_secs());
        let delivery_rate = if expected > 0 {
            final_stats.total_received as f64 / expected as f64
        } else {
            0.0
        };

        // Save final results
        let final_results = FinalResults {
            scenario_id,
            total_sent: final_stats.total_sent,
            total_received: final_stats.total_received,
            total_acked: final_stats.total_acked,
            total_timed_out: final_stats.total_timed_out,
            total_errors: final_stats.errors,
            avg_send_rate: final_stats.send_rate,
            avg_recv_rate: final_stats.receive_rate,
            delivery_rate,
            min_ns: final_percentiles.min_ns,
            max_ns: final_percentiles.max_ns,
            mean_ns: final_percentiles.mean_ns,
            stdev_ns: final_percentiles.stdev_ns,
            p50_ns: final_percentiles.p50_ns,
            p75_ns: final_percentiles.p75_ns,
            p90_ns: final_percentiles.p90_ns,
            p95_ns: final_percentiles.p95_ns,
            p99_ns: final_percentiles.p99_ns,
            p999_ns: final_percentiles.p999_ns,
            p9999_ns: final_percentiles.p9999_ns,
        };
        self.db.insert_final_results(&final_results)?;

        // Update end times
        let ended_at = Utc::now();
        self.db.update_scenario_ended(scenario_id, ended_at)?;
        self.db.update_run_ended(run_id, ended_at)?;

        // Stop Docker collection gracefully
        // Drop the sender side implicitly by not using it anymore
        // Wait a moment for any in-flight samples to be processed
        tokio::time::sleep(Duration::from_millis(100)).await;

        if let Some(handle) = docker_handle {
            handle.abort();
        }
        docker_receiver.abort();

        // Print summary
        println!();
        println!("═══════════════════════════════════════════════════════════════");
        println!("                      BENCHMARK RESULTS                         ");
        println!("═══════════════════════════════════════════════════════════════");
        println!();
        println!("  Scenario:        {} @ QoS {}", self.config.scenario, self.config.qos);
        println!("  Duration:        {:.1}s", duration_secs);
        println!("  Publishers:      {}", num_publishers);
        println!("  Subscribers:     {}", num_subscribers);
        println!("  Target Rate:     {} msg/s per publisher", self.config.rate);
        println!();

        // Connection stats
        let conn_attempted = metrics.counters.connections_attempted.load(Ordering::Acquire);
        let conn_succeeded = metrics.counters.connections_succeeded.load(Ordering::Acquire);
        let conn_failed = metrics.counters.connections_failed.load(Ordering::Acquire);

        println!("  CONNECTIONS");
        println!("  ──────────────────────────────────────────────────────────────");
        println!("  Attempted:         {:>12}", conn_attempted);
        println!("  Succeeded:         {:>12}", conn_succeeded);
        if conn_failed > 0 {
            println!("  Failed:            {:>12}  ⚠", conn_failed);
        } else {
            println!("  Failed:            {:>12}", conn_failed);
        }
        println!();
        println!("  THROUGHPUT");
        println!("  ──────────────────────────────────────────────────────────────");
        println!("  Messages Sent:     {:>12}", format_count(final_stats.total_sent));
        println!("  Messages Received: {:>12}", format_count(final_stats.total_received));
        if final_stats.total_timed_out > 0 {
            println!("  Messages Timed Out:{:>12}  ⚠", format_count(final_stats.total_timed_out));
        }
        if final_stats.errors > 0 {
            println!("  Errors:            {:>12}  ⚠", format_count(final_stats.errors));
        }
        println!("  Avg Send Rate:     {:>12.0} msg/s", final_stats.send_rate);
        println!("  Avg Recv Rate:     {:>12.0} msg/s", final_stats.receive_rate);
        println!("  Delivery Rate:     {:>12.2}%", delivery_rate * 100.0);
        println!();
        println!("  LATENCY");
        println!("  ──────────────────────────────────────────────────────────────");
        println!("  Min:               {:>12}", format_latency_ns(final_percentiles.min_ns));
        println!("  Mean:              {:>12}", format_latency_ns(final_percentiles.mean_ns as u64));
        println!("  P50 (median):      {:>12}", format_latency_ns(final_percentiles.p50_ns));
        println!("  P90:               {:>12}", format_latency_ns(final_percentiles.p90_ns));
        println!("  P95:               {:>12}", format_latency_ns(final_percentiles.p95_ns));
        println!("  P99:               {:>12}", format_latency_ns(final_percentiles.p99_ns));
        println!("  P99.9:             {:>12}", format_latency_ns(final_percentiles.p999_ns));
        println!("  P99.99:            {:>12}", format_latency_ns(final_percentiles.p9999_ns));
        println!("  Max:               {:>12}", format_latency_ns(final_percentiles.max_ns));
        println!();
        // Save container samples to database and show summary
        let samples = container_samples.lock().unwrap();
        for sample in samples.iter() {
            if let Err(e) = self.db.insert_container_sample(run_id, sample) {
                debug!("Failed to insert container sample: {}", e);
            }
        }

        if !samples.is_empty() {
            debug!(
                sample_count = samples.len(),
                "Processing container samples"
            );

            // CPU percentages are pre-calculated using precpu_stats (like `docker stats`)
            let cpu_percentages: Vec<f64> = samples.iter()
                .map(|s| s.cpu_percent)
                .filter(|&p| p > 0.0)
                .collect();

            let avg_cpu = if !cpu_percentages.is_empty() {
                cpu_percentages.iter().sum::<f64>() / cpu_percentages.len() as f64
            } else {
                0.0
            };
            let max_cpu = cpu_percentages.iter().cloned().fold(0.0, f64::max);

            // Memory stats
            let avg_memory_mb = samples.iter()
                .map(|s| s.memory_usage_bytes as f64 / 1_000_000.0)
                .sum::<f64>() / samples.len() as f64;
            let max_memory_mb = samples.iter()
                .map(|s| s.memory_usage_bytes)
                .max()
                .unwrap_or(0) as f64 / 1_000_000.0;
            let memory_limit_mb = samples.first()
                .map(|s| s.memory_limit_bytes as f64 / 1_000_000.0)
                .unwrap_or(0.0);

            // Network stats (delta between first and last)
            if let (Some(first), Some(last)) = (samples.first(), samples.last()) {
                let rx_bytes = last.network_rx_bytes.saturating_sub(first.network_rx_bytes);
                let tx_bytes = last.network_tx_bytes.saturating_sub(first.network_tx_bytes);

                debug!(
                    samples = samples.len(),
                    avg_cpu = format!("{:.1}%", avg_cpu),
                    max_cpu = format!("{:.1}%", max_cpu),
                    rx_delta = rx_bytes,
                    tx_delta = tx_bytes,
                    "Container stats summary"
                );

                println!("  CONTAINER RESOURCES");
                println!("  ──────────────────────────────────────────────────────────────");
                println!("  CPU Avg:           {:>12.1}%", avg_cpu);
                println!("  CPU Max:           {:>12.1}%", max_cpu);
                println!("  Memory Avg:        {:>12.1} MB", avg_memory_mb);
                println!("  Memory Max:        {:>12.1} MB", max_memory_mb);
                if memory_limit_mb > 0.0 {
                    println!("  Memory Limit:      {:>12.1} MB", memory_limit_mb);
                }

                // Show delta if available, otherwise show cumulative from last sample
                // (delta might be 0 if container uses host networking)
                if rx_bytes > 0 || tx_bytes > 0 {
                    println!("  Network RX:        {:>12}", format_bytes(rx_bytes));
                    println!("  Network TX:        {:>12}", format_bytes(tx_bytes));
                } else if last.network_rx_bytes > 0 || last.network_tx_bytes > 0 {
                    // Show cumulative if deltas are 0 but there's data
                    println!("  Network RX (total):{:>12}", format_bytes(last.network_rx_bytes));
                    println!("  Network TX (total):{:>12}", format_bytes(last.network_tx_bytes));
                } else {
                    println!("  Network:           N/A (host networking or unavailable)");
                }
                println!();
            }
        }

        println!("  Run ID: {}", run_id);
        println!("  Scenario ID: {}", scenario_id);
        println!();

        // Show helpful hints if there were issues
        if conn_failed > 0 {
            println!("  ⚠ {} connection(s) failed. If you see 'Too many open files',", conn_failed);
            println!("    increase your file descriptor limit: ulimit -n 65535");
            println!();
        }

        Ok(())
    }
}

/// Format a message count with suffixes
fn format_count(count: u64) -> String {
    if count >= 1_000_000_000 {
        format!("{:.2}B", count as f64 / 1_000_000_000.0)
    } else if count >= 1_000_000 {
        format!("{:.2}M", count as f64 / 1_000_000.0)
    } else if count >= 1_000 {
        format!("{:.2}K", count as f64 / 1_000.0)
    } else {
        format!("{}", count)
    }
}

/// Format latency in nanoseconds to a human-readable string
fn format_latency_ns(ns: u64) -> String {
    if ns >= 1_000_000_000 {
        format!("{:.2}s", ns as f64 / 1_000_000_000.0)
    } else if ns >= 1_000_000 {
        format!("{:.2}ms", ns as f64 / 1_000_000.0)
    } else if ns >= 1_000 {
        format!("{:.2}µs", ns as f64 / 1_000.0)
    } else {
        format!("{}ns", ns)
    }
}

/// Format bytes to human-readable string
fn format_bytes(bytes: u64) -> String {
    if bytes >= 1_000_000_000 {
        format!("{:.2} GB", bytes as f64 / 1_000_000_000.0)
    } else if bytes >= 1_000_000 {
        format!("{:.2} MB", bytes as f64 / 1_000_000.0)
    } else if bytes >= 1_000 {
        format!("{:.2} KB", bytes as f64 / 1_000.0)
    } else {
        format!("{} B", bytes)
    }
}
