use parking_lot::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Latency histogram buckets in nanoseconds (finer granularity, especially for higher latencies)
pub const BUCKETS_NS: &[u64] = &[
    100_000,         // 100µs
    250_000,         // 250µs
    500_000,         // 500µs
    1_000_000,       // 1ms
    2_500_000,       // 2.5ms
    5_000_000,       // 5ms
    10_000_000,      // 10ms
    25_000_000,      // 25ms
    50_000_000,      // 50ms
    100_000_000,     // 100ms
    250_000_000,     // 250ms
    500_000_000,     // 500ms
    750_000_000,     // 750ms
    1_000_000_000,   // 1s
    1_500_000_000,   // 1.5s
    2_000_000_000,   // 2s
    2_500_000_000,   // 2.5s
    3_000_000_000,   // 3s
    4_000_000_000,   // 4s
    5_000_000_000,   // 5s
    6_000_000_000,   // 6s
    7_000_000_000,   // 7s
    8_000_000_000,   // 8s
    9_000_000_000,   // 9s
    10_000_000_000,  // 10s
    u64::MAX,        // +Inf
];

/// Number of buckets
pub const NUM_BUCKETS: usize = 26;

/// Bucket names for database columns
pub const BUCKET_NAMES: &[&str] = &[
    "bucket_100us",
    "bucket_250us",
    "bucket_500us",
    "bucket_1ms",
    "bucket_2_5ms",
    "bucket_5ms",
    "bucket_10ms",
    "bucket_25ms",
    "bucket_50ms",
    "bucket_100ms",
    "bucket_250ms",
    "bucket_500ms",
    "bucket_750ms",
    "bucket_1s",
    "bucket_1_5s",
    "bucket_2s",
    "bucket_2_5s",
    "bucket_3s",
    "bucket_4s",
    "bucket_5s",
    "bucket_6s",
    "bucket_7s",
    "bucket_8s",
    "bucket_9s",
    "bucket_10s",
    "bucket_inf",
];

/// Atomic counters for throughput metrics
#[derive(Debug, Default)]
pub struct Counters {
    pub messages_sent: AtomicU64,
    pub messages_received: AtomicU64,
    pub messages_acked: AtomicU64,
    pub messages_timed_out: AtomicU64,
    pub bytes_sent: AtomicU64,
    pub bytes_received: AtomicU64,
    pub errors: AtomicU64,
    pub connections_attempted: AtomicU64,
    pub connections_succeeded: AtomicU64,
    pub connections_failed: AtomicU64,
    pub reconnections: AtomicU64,
}

impl Counters {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn snapshot(&self) -> CounterSnapshot {
        CounterSnapshot {
            messages_sent: self.messages_sent.load(Ordering::Acquire),
            messages_received: self.messages_received.load(Ordering::Acquire),
            messages_acked: self.messages_acked.load(Ordering::Acquire),
            messages_timed_out: self.messages_timed_out.load(Ordering::Acquire),
            bytes_sent: self.bytes_sent.load(Ordering::Acquire),
            bytes_received: self.bytes_received.load(Ordering::Acquire),
            errors: self.errors.load(Ordering::Acquire),
        }
    }

    pub fn inc_sent(&self, bytes: u64) {
        self.messages_sent.fetch_add(1, Ordering::Relaxed);
        self.bytes_sent.fetch_add(bytes, Ordering::Relaxed);
    }

    pub fn inc_received(&self, bytes: u64) {
        self.messages_received.fetch_add(1, Ordering::Relaxed);
        self.bytes_received.fetch_add(bytes, Ordering::Relaxed);
    }

    pub fn inc_acked(&self) {
        self.messages_acked.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_errors(&self) {
        self.errors.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_timed_out(&self, count: u64) {
        self.messages_timed_out.fetch_add(count, Ordering::Relaxed);
    }

    pub fn inc_connection_attempt(&self) {
        self.connections_attempted.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_connection_success(&self) {
        self.connections_succeeded.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_connection_failed(&self) {
        self.connections_failed.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_reconnection(&self) {
        self.reconnections.fetch_add(1, Ordering::Relaxed);
    }

    pub fn reset(&self) {
        self.messages_sent.store(0, Ordering::Release);
        self.messages_received.store(0, Ordering::Release);
        self.messages_acked.store(0, Ordering::Release);
        self.messages_timed_out.store(0, Ordering::Release);
        self.bytes_sent.store(0, Ordering::Release);
        self.bytes_received.store(0, Ordering::Release);
        self.errors.store(0, Ordering::Release);
        // Don't reset connection counters - they represent total connections
    }
}

#[derive(Debug, Clone, Default)]
pub struct CounterSnapshot {
    pub messages_sent: u64,
    pub messages_received: u64,
    pub messages_acked: u64,
    pub messages_timed_out: u64,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub errors: u64,
}

/// Histogram accumulator using atomic operations for lock-free concurrent updates
#[derive(Debug)]
pub struct HistogramAccumulator {
    buckets: [AtomicU64; NUM_BUCKETS],
    sum_ns: AtomicU64,
    count: AtomicU64,
    min_ns: AtomicU64,
    max_ns: AtomicU64,
}

impl Default for HistogramAccumulator {
    fn default() -> Self {
        Self::new()
    }
}

impl HistogramAccumulator {
    pub fn new() -> Self {
        Self {
            buckets: Default::default(),
            sum_ns: AtomicU64::new(0),
            count: AtomicU64::new(0),
            min_ns: AtomicU64::new(u64::MAX),
            max_ns: AtomicU64::new(0),
        }
    }

    /// Record a latency observation
    pub fn observe(&self, latency_ns: u64) {
        // Find the bucket index
        let bucket_idx = BUCKETS_NS
            .iter()
            .position(|&b| latency_ns <= b)
            .unwrap_or(BUCKETS_NS.len() - 1);

        // Cumulative: increment this bucket and all higher buckets
        for i in bucket_idx..self.buckets.len() {
            self.buckets[i].fetch_add(1, Ordering::Relaxed);
        }

        self.sum_ns.fetch_add(latency_ns, Ordering::Relaxed);
        self.count.fetch_add(1, Ordering::Relaxed);

        // Update min (compare-and-swap loop)
        let mut current_min = self.min_ns.load(Ordering::Relaxed);
        while latency_ns < current_min {
            match self.min_ns.compare_exchange_weak(
                current_min,
                latency_ns,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(x) => current_min = x,
            }
        }

        // Update max (compare-and-swap loop)
        let mut current_max = self.max_ns.load(Ordering::Relaxed);
        while latency_ns > current_max {
            match self.max_ns.compare_exchange_weak(
                current_max,
                latency_ns,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(x) => current_max = x,
            }
        }
    }

    /// Take a snapshot of the current histogram state
    pub fn snapshot(&self) -> HistogramSnapshot {
        let mut buckets = [0u64; NUM_BUCKETS];
        for (i, bucket) in self.buckets.iter().enumerate() {
            buckets[i] = bucket.load(Ordering::Acquire);
        }

        let min = self.min_ns.load(Ordering::Acquire);
        let max = self.max_ns.load(Ordering::Acquire);

        HistogramSnapshot {
            buckets,
            sum_ns: self.sum_ns.load(Ordering::Acquire),
            count: self.count.load(Ordering::Acquire),
            min_ns: if min == u64::MAX { None } else { Some(min) },
            max_ns: if max == 0 { None } else { Some(max) },
        }
    }

    /// Reset the histogram (for interval-based reporting)
    pub fn reset(&self) {
        for bucket in &self.buckets {
            bucket.store(0, Ordering::Release);
        }
        self.sum_ns.store(0, Ordering::Release);
        self.count.store(0, Ordering::Release);
        self.min_ns.store(u64::MAX, Ordering::Release);
        self.max_ns.store(0, Ordering::Release);
    }
}

#[derive(Debug, Clone)]
pub struct HistogramSnapshot {
    pub buckets: [u64; NUM_BUCKETS],
    pub sum_ns: u64,
    pub count: u64,
    pub min_ns: Option<u64>,
    pub max_ns: Option<u64>,
}

impl Default for HistogramSnapshot {
    fn default() -> Self {
        Self {
            buckets: [0; NUM_BUCKETS],
            sum_ns: 0,
            count: 0,
            min_ns: None,
            max_ns: None,
        }
    }
}

#[allow(dead_code)]
impl HistogramSnapshot {
    /// Calculate the mean latency in nanoseconds
    pub fn mean_ns(&self) -> Option<f64> {
        if self.count > 0 {
            Some(self.sum_ns as f64 / self.count as f64)
        } else {
            None
        }
    }

    /// Estimate a percentile from the histogram buckets using linear interpolation
    /// This follows the Prometheus histogram_quantile() approach
    pub fn percentile_ns(&self, percentile: f64) -> Option<u64> {
        if self.count == 0 {
            return None;
        }

        let target_rank = self.count as f64 * percentile / 100.0;

        // Find the bucket containing the target rank
        let mut prev_count = 0u64;
        let mut prev_bound = 0u64;

        for (i, &bucket_count) in self.buckets.iter().enumerate() {
            if bucket_count as f64 >= target_rank {
                let bucket_bound = BUCKETS_NS[i];

                // Handle edge case: first bucket or no samples in lower buckets
                if prev_count == bucket_count || i == 0 {
                    return Some(bucket_bound);
                }

                // Linear interpolation within the bucket
                // fraction = how far into this bucket's range the percentile falls
                let bucket_fraction = (target_rank - prev_count as f64)
                    / (bucket_count - prev_count) as f64;

                // Interpolate between previous bound and current bound
                let interpolated = prev_bound as f64
                    + bucket_fraction * (bucket_bound - prev_bound) as f64;

                return Some(interpolated as u64);
            }
            prev_count = bucket_count;
            prev_bound = BUCKETS_NS[i];
        }

        // All samples are in the +Inf bucket
        self.max_ns
    }

    /// Get P50 (median)
    pub fn p50_ns(&self) -> Option<u64> {
        self.percentile_ns(50.0)
    }

    /// Get P95
    pub fn p95_ns(&self) -> Option<u64> {
        self.percentile_ns(95.0)
    }

    /// Get P99
    pub fn p99_ns(&self) -> Option<u64> {
        self.percentile_ns(99.0)
    }

    /// Get P99.9
    pub fn p999_ns(&self) -> Option<u64> {
        self.percentile_ns(99.9)
    }

    /// Merge another histogram snapshot into this one
    pub fn merge(&mut self, other: &HistogramSnapshot) {
        for (i, count) in other.buckets.iter().enumerate() {
            self.buckets[i] += count;
        }
        self.sum_ns += other.sum_ns;
        self.count += other.count;

        if let Some(other_min) = other.min_ns {
            self.min_ns = Some(self.min_ns.map_or(other_min, |m| m.min(other_min)));
        }
        if let Some(other_max) = other.max_ns {
            self.max_ns = Some(self.max_ns.map_or(other_max, |m| m.max(other_max)));
        }
    }
}

/// Combined metrics collector with exact HDR histogram for percentiles
#[derive(Clone)]
pub struct MetricsCollector {
    pub counters: Arc<Counters>,
    /// Bucket-based histogram for efficient DB storage
    pub bucket_histogram: Arc<HistogramAccumulator>,
    /// HDR histogram for exact percentile calculations
    pub hdr_histogram: Arc<HdrHistogram>,
    start_time: Arc<Mutex<Instant>>,
}

#[allow(dead_code)]
impl MetricsCollector {
    pub fn new() -> Self {
        Self {
            counters: Arc::new(Counters::new()),
            bucket_histogram: Arc::new(HistogramAccumulator::new()),
            hdr_histogram: Arc::new(HdrHistogram::new().expect("Failed to create HDR histogram")),
            start_time: Arc::new(Mutex::new(Instant::now())),
        }
    }

    /// Record a latency observation (updates both bucket and HDR histograms)
    pub fn record_latency(&self, latency_ns: u64) {
        self.bucket_histogram.observe(latency_ns);
        self.hdr_histogram.record(latency_ns);
    }

    pub fn elapsed(&self) -> Duration {
        self.start_time.lock().elapsed()
    }

    pub fn elapsed_ns(&self) -> u64 {
        self.start_time.lock().elapsed().as_nanos() as u64
    }

    pub fn reset_start_time(&self) {
        *self.start_time.lock() = Instant::now();
    }

    /// Reset all metrics (used after warmup)
    pub fn reset(&self) {
        self.counters.reset();
        self.bucket_histogram.reset();
        self.hdr_histogram.reset();
    }

    /// Get exact percentiles from HDR histogram
    pub fn exact_percentiles(&self) -> ExactPercentiles {
        self.hdr_histogram.exact_percentiles()
    }

    /// Get bucket histogram snapshot for DB storage
    pub fn bucket_snapshot(&self) -> HistogramSnapshot {
        self.bucket_histogram.snapshot()
    }

    /// Get current throughput statistics
    pub fn throughput_stats(&self) -> ThroughputStats {
        let elapsed = self.start_time.lock().elapsed();
        let snapshot = self.counters.snapshot();

        let elapsed_secs = elapsed.as_secs_f64();
        if elapsed_secs <= 0.0 {
            return ThroughputStats::default();
        }

        ThroughputStats {
            send_rate: snapshot.messages_sent as f64 / elapsed_secs,
            receive_rate: snapshot.messages_received as f64 / elapsed_secs,
            total_sent: snapshot.messages_sent,
            total_received: snapshot.messages_received,
            total_acked: snapshot.messages_acked,
            total_timed_out: snapshot.messages_timed_out,
            errors: snapshot.errors,
        }
    }

    /// Get current P99 in milliseconds (for progress display)
    pub fn current_p99_ms(&self) -> f64 {
        self.hdr_histogram.percentile_ms(99.0)
    }
}

impl Default for MetricsCollector {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Default)]
pub struct ThroughputStats {
    pub send_rate: f64,
    pub receive_rate: f64,
    pub total_sent: u64,
    pub total_received: u64,
    pub total_acked: u64,
    pub total_timed_out: u64,
    pub errors: u64,
}

/// HDR histogram wrapper for precise percentile calculations
/// This is the PRIMARY source for accurate latency percentiles
pub struct HdrHistogram {
    inner: Mutex<hdrhistogram::Histogram<u64>>,
}

#[allow(dead_code)]
impl HdrHistogram {
    pub fn new() -> anyhow::Result<Self> {
        // 1µs to 60s with 3 significant figures (0.1% precision)
        // This gives us exact millisecond-level precision for latencies up to 60s
        let histogram = hdrhistogram::Histogram::new_with_bounds(1_000, 60_000_000_000, 3)?;
        Ok(Self {
            inner: Mutex::new(histogram),
        })
    }

    pub fn record(&self, latency_ns: u64) {
        let mut hist = self.inner.lock();
        let _ = hist.record(latency_ns.max(1_000)); // Clamp to minimum 1µs
    }

    /// Get exact percentile value in nanoseconds
    pub fn percentile_ns(&self, percentile: f64) -> u64 {
        self.inner.lock().value_at_percentile(percentile)
    }

    /// Get exact percentile value in microseconds
    pub fn percentile_us(&self, percentile: f64) -> u64 {
        self.percentile_ns(percentile) / 1_000
    }

    /// Get exact percentile value in milliseconds
    pub fn percentile_ms(&self, percentile: f64) -> f64 {
        self.percentile_ns(percentile) as f64 / 1_000_000.0
    }

    pub fn mean_ns(&self) -> f64 {
        self.inner.lock().mean()
    }

    pub fn mean_ms(&self) -> f64 {
        self.mean_ns() / 1_000_000.0
    }

    pub fn min_ns(&self) -> u64 {
        self.inner.lock().min()
    }

    pub fn max_ns(&self) -> u64 {
        self.inner.lock().max()
    }

    pub fn count(&self) -> u64 {
        self.inner.lock().len()
    }

    pub fn stdev_ns(&self) -> f64 {
        self.inner.lock().stdev()
    }

    pub fn reset(&self) {
        self.inner.lock().reset();
    }

    /// Get a full snapshot of exact percentiles
    pub fn exact_percentiles(&self) -> ExactPercentiles {
        let hist = self.inner.lock();
        ExactPercentiles {
            min_ns: hist.min(),
            max_ns: hist.max(),
            mean_ns: hist.mean(),
            stdev_ns: hist.stdev(),
            p50_ns: hist.value_at_percentile(50.0),
            p75_ns: hist.value_at_percentile(75.0),
            p90_ns: hist.value_at_percentile(90.0),
            p95_ns: hist.value_at_percentile(95.0),
            p99_ns: hist.value_at_percentile(99.0),
            p999_ns: hist.value_at_percentile(99.9),
            p9999_ns: hist.value_at_percentile(99.99),
            count: hist.len(),
        }
    }

    /// Convert to bucket counts for storage (lossy, for DB efficiency)
    pub fn to_bucket_counts(&self) -> [u64; NUM_BUCKETS] {
        let hist = self.inner.lock();
        let mut buckets = [0u64; NUM_BUCKETS];

        // Count values at or below each bucket boundary
        for (i, &bound) in BUCKETS_NS.iter().enumerate() {
            buckets[i] = hist.count_between(0, bound);
        }

        buckets
    }
}

impl Default for HdrHistogram {
    fn default() -> Self {
        Self::new().expect("Failed to create HDR histogram")
    }
}

/// Exact percentile values from HDR histogram
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct ExactPercentiles {
    pub min_ns: u64,
    pub max_ns: u64,
    pub mean_ns: f64,
    pub stdev_ns: f64,
    pub p50_ns: u64,
    pub p75_ns: u64,
    pub p90_ns: u64,
    pub p95_ns: u64,
    pub p99_ns: u64,
    pub p999_ns: u64,
    pub p9999_ns: u64,
    pub count: u64,
}

#[allow(dead_code)]
impl ExactPercentiles {
    /// Convert nanoseconds to milliseconds for display
    pub fn p50_ms(&self) -> f64 {
        self.p50_ns as f64 / 1_000_000.0
    }

    pub fn p95_ms(&self) -> f64 {
        self.p95_ns as f64 / 1_000_000.0
    }

    pub fn p99_ms(&self) -> f64 {
        self.p99_ns as f64 / 1_000_000.0
    }

    pub fn p999_ms(&self) -> f64 {
        self.p999_ns as f64 / 1_000_000.0
    }

    pub fn mean_ms(&self) -> f64 {
        self.mean_ns / 1_000_000.0
    }

    pub fn min_ms(&self) -> f64 {
        self.min_ns as f64 / 1_000_000.0
    }

    pub fn max_ms(&self) -> f64 {
        self.max_ns as f64 / 1_000_000.0
    }
}
