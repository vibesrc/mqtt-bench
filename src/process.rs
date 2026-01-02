use crate::docker::ContainerSample;
use anyhow::{Context, Result};
use chrono::Utc;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

/// Process stats collector configuration
#[derive(Clone)]
pub struct ProcessStatsConfig {
    pub pid: u32,
    pub name: String,
    pub interval: Duration,
}

/// Process stats collector - reads from /proc on Linux
pub struct ProcessStatsCollector {
    config: ProcessStatsConfig,
    stop: Arc<AtomicBool>,
    samples_tx: mpsc::Sender<ContainerSample>,
    start_time: Instant,
    clock_ticks_per_sec: u64,
    num_cpus: u32,
    total_memory: u64,
    prev_cpu_total: u64,
    prev_system_total: u64,
}

impl ProcessStatsCollector {
    /// Create a new process stats collector
    pub fn new(
        config: ProcessStatsConfig,
        stop: Arc<AtomicBool>,
        samples_tx: mpsc::Sender<ContainerSample>,
    ) -> Result<Self> {
        // Get clock ticks per second (usually 100 on Linux)
        let clock_ticks_per_sec = unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as u64;

        // Get number of CPUs
        let num_cpus = num_cpus::get() as u32;

        // Get total system memory
        let total_memory = get_total_memory()?;

        // Verify process exists
        let proc_path = format!("/proc/{}", config.pid);
        if !Path::new(&proc_path).exists() {
            anyhow::bail!("Process {} not found", config.pid);
        }

        info!(
            pid = config.pid,
            name = %config.name,
            "Monitoring local process for resource usage"
        );

        Ok(Self {
            config,
            stop,
            samples_tx,
            start_time: Instant::now(),
            clock_ticks_per_sec,
            num_cpus,
            total_memory,
            prev_cpu_total: 0,
            prev_system_total: 0,
        })
    }

    /// Try to create a collector, returning None if unavailable
    pub fn try_new(
        config: ProcessStatsConfig,
        stop: Arc<AtomicBool>,
        samples_tx: mpsc::Sender<ContainerSample>,
    ) -> Option<Self> {
        match Self::new(config, stop, samples_tx) {
            Ok(collector) => Some(collector),
            Err(e) => {
                warn!("Process stats collection unavailable: {}", e);
                None
            }
        }
    }

    /// Run the stats collection loop
    pub async fn run(mut self, phase_fn: impl Fn(Duration) -> String + Send + 'static) -> Result<()> {
        let mut interval = tokio::time::interval(self.config.interval);

        // Initial read to get baseline
        if let Ok((cpu, sys)) = self.read_cpu_times() {
            self.prev_cpu_total = cpu;
            self.prev_system_total = sys;
        }

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
                        break;
                    }
                }
                Err(e) => {
                    debug!(error = %e, "Failed to collect process stats");
                }
            }
        }

        Ok(())
    }

    /// Read CPU times from /proc/{pid}/stat
    fn read_cpu_times(&self) -> Result<(u64, u64)> {
        let stat_path = format!("/proc/{}/stat", self.config.pid);
        let stat = fs::read_to_string(&stat_path)
            .with_context(|| format!("Failed to read {}", stat_path))?;

        // Format: pid (comm) state ppid pgrp session tty_nr tpgid flags
        //         minflt cminflt majflt cmajflt utime stime cutime cstime ...
        // Fields are space-separated, but comm can contain spaces and is in parens
        let comm_end = stat.rfind(')').ok_or_else(|| anyhow::anyhow!("Invalid stat format"))?;
        let fields: Vec<&str> = stat[comm_end + 2..].split_whitespace().collect();

        // utime is field 11 (0-indexed after comm), stime is field 12
        // In the split after ")", index 11 = utime, index 12 = stime
        let utime: u64 = fields.get(11).and_then(|s| s.parse().ok()).unwrap_or(0);
        let stime: u64 = fields.get(12).and_then(|s| s.parse().ok()).unwrap_or(0);

        let process_cpu = utime + stime;

        // Get system-wide CPU time from /proc/stat
        let sys_stat = fs::read_to_string("/proc/stat")?;
        let cpu_line = sys_stat.lines().next().ok_or_else(|| anyhow::anyhow!("Empty /proc/stat"))?;
        let cpu_fields: Vec<u64> = cpu_line
            .split_whitespace()
            .skip(1) // Skip "cpu"
            .filter_map(|s| s.parse().ok())
            .collect();

        let system_total: u64 = cpu_fields.iter().sum();

        Ok((process_cpu, system_total))
    }

    /// Collect a single stats sample
    async fn collect_stats(&mut self, phase: &str) -> Result<ContainerSample> {
        // Read CPU times
        let (cpu_total, system_total) = self.read_cpu_times()?;

        // Calculate CPU percentage
        let cpu_delta = cpu_total.saturating_sub(self.prev_cpu_total);
        let system_delta = system_total.saturating_sub(self.prev_system_total);

        let cpu_percent = if system_delta > 0 {
            (cpu_delta as f64 / system_delta as f64) * (self.num_cpus as f64) * 100.0
        } else {
            0.0
        };

        self.prev_cpu_total = cpu_total;
        self.prev_system_total = system_total;

        // Convert to nanoseconds for compatibility with container samples
        let cpu_total_ns = cpu_total * 1_000_000_000 / self.clock_ticks_per_sec;
        let cpu_system_ns = system_total * 1_000_000_000 / self.clock_ticks_per_sec;

        // Read memory from /proc/{pid}/statm or /proc/{pid}/status
        let memory_usage_bytes = self.read_memory_rss()?;

        // Read network stats from /proc/{pid}/net/dev (usually same as host for local process)
        let (network_rx_bytes, network_tx_bytes) = self.read_network_stats().unwrap_or((0, 0));

        let rel_ns = self.start_time.elapsed().as_nanos() as u64;

        debug!(
            cpu_percent = format!("{:.1}%", cpu_percent),
            memory_mb = memory_usage_bytes / 1_000_000,
            "Process stats sample"
        );

        Ok(ContainerSample {
            rel_ns,
            abs_ts: Utc::now(),
            phase: phase.to_string(),
            cpu_percent,
            cpu_total_ns,
            cpu_system_ns,
            cpu_online: self.num_cpus,
            memory_usage_bytes,
            memory_limit_bytes: self.total_memory,
            network_rx_bytes,
            network_tx_bytes,
        })
    }

    /// Read RSS memory from /proc/{pid}/statm
    fn read_memory_rss(&self) -> Result<u64> {
        let statm_path = format!("/proc/{}/statm", self.config.pid);
        let statm = fs::read_to_string(&statm_path)?;

        // Format: size resident shared text lib data dt
        // resident is the second field, in pages
        let fields: Vec<&str> = statm.split_whitespace().collect();
        let resident_pages: u64 = fields.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);

        // Page size is typically 4096
        let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as u64;

        Ok(resident_pages * page_size)
    }

    /// Read network stats - for local processes, read from /proc/net/dev
    fn read_network_stats(&self) -> Result<(u64, u64)> {
        let net_dev = fs::read_to_string("/proc/net/dev")?;

        let mut rx_total = 0u64;
        let mut tx_total = 0u64;

        for line in net_dev.lines().skip(2) { // Skip header lines
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 10 {
                let iface = parts[0].trim_end_matches(':');
                // Skip loopback
                if iface != "lo" {
                    rx_total += parts[1].parse::<u64>().unwrap_or(0);
                    tx_total += parts[9].parse::<u64>().unwrap_or(0);
                }
            }
        }

        Ok((rx_total, tx_total))
    }
}

/// Get total system memory from /proc/meminfo
fn get_total_memory() -> Result<u64> {
    let meminfo = fs::read_to_string("/proc/meminfo")?;

    for line in meminfo.lines() {
        if line.starts_with("MemTotal:") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if let Some(kb_str) = parts.get(1) {
                let kb: u64 = kb_str.parse()?;
                return Ok(kb * 1024); // Convert to bytes
            }
        }
    }

    Ok(0)
}

/// Information about a detected process
#[derive(Debug, Clone)]
pub struct DetectedProcess {
    pub pid: u32,
    pub name: String,
}

/// Try to find a process listening on the specified port
pub fn detect_process_by_port(port: u16) -> Option<DetectedProcess> {
    // Read /proc/net/tcp to find the socket
    let tcp_data = fs::read_to_string("/proc/net/tcp").ok()?;
    let tcp6_data = fs::read_to_string("/proc/net/tcp6").ok();

    let target_port_hex = format!("{:04X}", port);

    // Find inode for the listening socket
    let mut socket_inode: Option<u64> = None;

    for line in tcp_data.lines().chain(tcp6_data.iter().flat_map(|s| s.lines())).skip(1) {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() >= 10 {
            // local_address is field 1, format is IP:PORT in hex
            let local_addr = fields[1];
            if let Some(port_hex) = local_addr.split(':').last() {
                // Check if listening (state 0A = LISTEN)
                if port_hex == target_port_hex && fields[3] == "0A" {
                    socket_inode = fields[9].parse().ok();
                    break;
                }
            }
        }
    }

    let inode = socket_inode?;

    // Search /proc/*/fd/* for the socket inode
    let proc_dir = fs::read_dir("/proc").ok()?;

    for entry in proc_dir.filter_map(|e| e.ok()) {
        let pid_str = entry.file_name().to_string_lossy().to_string();
        if let Ok(pid) = pid_str.parse::<u32>() {
            let fd_dir = format!("/proc/{}/fd", pid);
            if let Ok(fds) = fs::read_dir(&fd_dir) {
                for fd_entry in fds.filter_map(|e| e.ok()) {
                    if let Ok(link) = fs::read_link(fd_entry.path()) {
                        let link_str = link.to_string_lossy();
                        if link_str.contains(&format!("socket:[{}]", inode)) {
                            // Found the process!
                            let name = fs::read_to_string(format!("/proc/{}/comm", pid))
                                .unwrap_or_default()
                                .trim()
                                .to_string();

                            info!(
                                pid = pid,
                                name = %name,
                                port = port,
                                "Auto-detected local process on port"
                            );

                            return Some(DetectedProcess { pid, name });
                        }
                    }
                }
            }
        }
    }

    debug!("No local process found listening on port {}", port);
    None
}
