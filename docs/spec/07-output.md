# Section 7: Output Formats

## Overview

Results are stored in DuckDB using Prometheus-style aggregation: histogram buckets, sum, and count aggregated at fixed intervals. This keeps write rate constant regardless of message throughput.

## 7.1 Storage Philosophy

### Aggregate at Source, Not Storage

At 20K msg/s, storing raw samples = 1.2M rows/minute = disaster. Instead:

```
❌ Raw samples (doesn't scale)
   20,000 INSERT/sec → database explodes

✅ Prometheus-style buckets (constant rate)
   1 INSERT/sec with histogram buckets → sustainable
```

### Fixed Histogram Buckets

Latency buckets (in nanoseconds), exponentially distributed:

```rust
const BUCKETS_NS: &[u64] = &[
    100_000,        // 100µs
    250_000,        // 250µs
    500_000,        // 500µs
    1_000_000,      // 1ms
    2_500_000,      // 2.5ms
    5_000_000,      // 5ms
    10_000_000,     // 10ms
    25_000_000,     // 25ms
    50_000_000,     // 50ms
    100_000_000,    // 100ms
    250_000_000,    // 250ms
    500_000_000,    // 500ms
    1_000_000_000,  // 1s
    u64::MAX,       // +Inf
];
```

### Aggregation Interval (Checkpoint)

Default: **10 seconds**, configurable via `--checkpoint-interval`.

Each checkpoint produces one row with:
- Cumulative bucket counts (like Prometheus `_bucket`)
- Sum of all latencies (for mean calculation)
- Total count
- Min/max for the interval

```bash
# Default: 10s checkpoints
mqtt-bench run fan-in --duration 60
# Results: 6 histogram rows

# High-resolution: 1s checkpoints
mqtt-bench run fan-in --duration 60 --checkpoint-interval 1s
# Results: 60 histogram rows

# Low-resolution: 30s checkpoints (long benchmarks)
mqtt-bench run fan-in --duration 3600 --checkpoint-interval 30s
# Results: 120 histogram rows
```

| Interval | Rows/min | Use Case |
|----------|----------|----------|
| 1s | 60 | Debugging, latency spikes |
| **10s** | **6** | **Default, good balance** |
| 30s | 2 | Long-running benchmarks |
| 60s | 1 | Minimal overhead |

### Dual Timestamps

Every aggregation row includes:
- `rel_ns` — Nanoseconds since benchmark start
- `abs_ts` — Absolute wall clock timestamp

---

## 7.2 DuckDB Schema

### runs

Unchanged — one row per benchmark invocation.

```sql
CREATE TABLE runs (
    run_id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    started_at      TIMESTAMP NOT NULL,
    ended_at        TIMESTAMP,
    
    broker_host     VARCHAR NOT NULL,
    broker_port     INTEGER NOT NULL,
    broker_name     VARCHAR,
    broker_version  VARCHAR,
    
    container_id    VARCHAR,
    container_name  VARCHAR,
    
    hostname        VARCHAR,
    cpus            INTEGER,
    memory_bytes    BIGINT,
    
    git_commit      VARCHAR,
    notes           VARCHAR
);
```

### scenarios

Unchanged — one row per scenario execution.

```sql
CREATE TABLE scenarios (
    scenario_id     UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    run_id          UUID REFERENCES runs(run_id),
    started_at      TIMESTAMP NOT NULL,
    ended_at        TIMESTAMP,
    
    scenario_name   VARCHAR NOT NULL,
    qos             INTEGER NOT NULL,
    publishers      INTEGER NOT NULL,
    subscribers     INTEGER NOT NULL,
    topics          INTEGER NOT NULL,
    msg_rate        INTEGER NOT NULL,
    payload_size    INTEGER NOT NULL,
    warmup_ns       BIGINT NOT NULL,
    duration_ns     BIGINT NOT NULL
);
```

### latency_histograms

**Pre-aggregated histogram buckets per interval.** One row per second.

```sql
CREATE TABLE latency_histograms (
    scenario_id     UUID REFERENCES scenarios(scenario_id),
    
    -- Timing
    rel_ns          BIGINT NOT NULL,    -- interval start (ns since scenario start)
    abs_ts          TIMESTAMP NOT NULL,
    
    -- Histogram buckets (cumulative counts, Prometheus-style)
    -- bucket_100us = count of samples <= 100µs
    -- bucket_250us = count of samples <= 250µs (includes bucket_100us)
    -- ... etc, each bucket includes all smaller buckets
    bucket_100us    BIGINT NOT NULL DEFAULT 0,
    bucket_250us    BIGINT NOT NULL DEFAULT 0,
    bucket_500us    BIGINT NOT NULL DEFAULT 0,
    bucket_1ms      BIGINT NOT NULL DEFAULT 0,
    bucket_2_5ms    BIGINT NOT NULL DEFAULT 0,
    bucket_5ms      BIGINT NOT NULL DEFAULT 0,
    bucket_10ms     BIGINT NOT NULL DEFAULT 0,
    bucket_25ms     BIGINT NOT NULL DEFAULT 0,
    bucket_50ms     BIGINT NOT NULL DEFAULT 0,
    bucket_100ms    BIGINT NOT NULL DEFAULT 0,
    bucket_250ms    BIGINT NOT NULL DEFAULT 0,
    bucket_500ms    BIGINT NOT NULL DEFAULT 0,
    bucket_1s       BIGINT NOT NULL DEFAULT 0,
    bucket_inf      BIGINT NOT NULL DEFAULT 0,  -- +Inf, equals count
    
    -- For mean calculation
    sum_ns          BIGINT NOT NULL DEFAULT 0,  -- sum of all latencies
    count           BIGINT NOT NULL DEFAULT 0,  -- total observations
    
    -- Min/max for the interval
    min_ns          BIGINT,
    max_ns          BIGINT
);
```

### throughput_counters

Cumulative counters per interval. One row per second.

```sql
CREATE TABLE throughput_counters (
    scenario_id     UUID REFERENCES scenarios(scenario_id),
    
    rel_ns          BIGINT NOT NULL,
    abs_ts          TIMESTAMP NOT NULL,
    
    -- Cumulative counters (monotonically increasing)
    messages_sent   BIGINT NOT NULL,
    messages_recv   BIGINT NOT NULL,
    messages_acked  BIGINT NOT NULL,
    bytes_sent      BIGINT NOT NULL,
    bytes_recv      BIGINT NOT NULL,
    errors          BIGINT NOT NULL
);
```

### container_samples

Container stats per interval. One row per second.

```sql
CREATE TABLE container_samples (
    run_id          UUID REFERENCES runs(run_id),
    
    rel_ns          BIGINT NOT NULL,
    abs_ts          TIMESTAMP NOT NULL,
    phase           VARCHAR NOT NULL,
    
    -- Raw cumulative values (compute deltas in queries)
    cpu_total_ns        BIGINT NOT NULL,
    cpu_system_ns       BIGINT NOT NULL,
    cpu_online          INTEGER NOT NULL,
    
    memory_usage_bytes  BIGINT NOT NULL,
    memory_limit_bytes  BIGINT NOT NULL,
    
    network_rx_bytes    BIGINT NOT NULL,
    network_tx_bytes    BIGINT NOT NULL
);
```

---

## 7.3 In-Memory Aggregation

### Histogram Accumulator

Each subscriber maintains a thread-local histogram, flushed to shared state each interval:

```rust
struct HistogramAccumulator {
    buckets: [AtomicU64; 14],  // one per bucket
    sum_ns: AtomicU64,
    count: AtomicU64,
    min_ns: AtomicU64,
    max_ns: AtomicU64,
}

impl HistogramAccumulator {
    fn observe(&self, latency_ns: u64) {
        // Increment appropriate bucket
        let bucket_idx = BUCKETS_NS.iter()
            .position(|&b| latency_ns <= b)
            .unwrap_or(BUCKETS_NS.len() - 1);
        
        // Cumulative: increment this bucket and all higher
        for i in bucket_idx..self.buckets.len() {
            self.buckets[i].fetch_add(1, Ordering::Relaxed);
        }
        
        self.sum_ns.fetch_add(latency_ns, Ordering::Relaxed);
        self.count.fetch_add(1, Ordering::Relaxed);
        self.min_ns.fetch_min(latency_ns, Ordering::Relaxed);
        self.max_ns.fetch_max(latency_ns, Ordering::Relaxed);
    }
    
    fn snapshot_and_reset(&self) -> HistogramSnapshot { ... }
}
```

### Checkpoint Flush

Every checkpoint interval (default 10s), a background task:

1. Snapshots all accumulators
2. Writes one row to `latency_histograms`
3. Writes one row to `throughput_counters`
4. Resets interval-specific values (min/max), keeps cumulative totals

### Write Rate

At 10-second intervals (default):
- `latency_histograms`: 6 rows/min
- `throughput_counters`: 6 rows/min
- `container_samples`: 6 rows/min (same interval)

**60-second benchmark = 18 total rows** regardless of message rate.

---

## 7.4 Query Patterns

### Percentile Estimation from Buckets

Prometheus-style histogram interpolation:

```sql
-- Estimate P99 from histogram buckets
WITH histogram AS (
    SELECT 
        count AS total,
        bucket_100us, bucket_250us, bucket_500us, bucket_1ms,
        bucket_2_5ms, bucket_5ms, bucket_10ms, bucket_25ms,
        bucket_50ms, bucket_100ms, bucket_250ms, bucket_500ms,
        bucket_1s, bucket_inf
    FROM latency_histograms
    WHERE scenario_id = ?
),
target AS (
    SELECT total * 0.99 AS target_count FROM histogram
)
SELECT 
    CASE 
        WHEN bucket_100us >= target_count THEN 100000
        WHEN bucket_250us >= target_count THEN 250000
        WHEN bucket_500us >= target_count THEN 500000
        WHEN bucket_1ms >= target_count THEN 1000000
        WHEN bucket_2_5ms >= target_count THEN 2500000
        WHEN bucket_5ms >= target_count THEN 5000000
        WHEN bucket_10ms >= target_count THEN 10000000
        WHEN bucket_25ms >= target_count THEN 25000000
        WHEN bucket_50ms >= target_count THEN 50000000
        WHEN bucket_100ms >= target_count THEN 100000000
        WHEN bucket_250ms >= target_count THEN 250000000
        WHEN bucket_500ms >= target_count THEN 500000000
        WHEN bucket_1s >= target_count THEN 1000000000
        ELSE 1000000001  -- >1s
    END AS p99_ns
FROM histogram, target;
```

### Aggregate Across Full Scenario

```sql
-- Sum all histogram intervals for final percentiles
SELECT 
    sum(bucket_100us) AS bucket_100us,
    sum(bucket_250us) AS bucket_250us,
    -- ... etc
    sum(sum_ns) AS total_sum_ns,
    sum(count) AS total_count,
    min(min_ns) AS global_min_ns,
    max(max_ns) AS global_max_ns,
    sum(sum_ns)::DOUBLE / sum(count) AS mean_ns
FROM latency_histograms
WHERE scenario_id = ?;
```

### Throughput Rate Over Time

```sql
-- Instantaneous rate per interval
WITH deltas AS (
    SELECT 
        rel_ns,
        messages_recv - lag(messages_recv) OVER (ORDER BY rel_ns) AS recv_delta,
        (rel_ns - lag(rel_ns) OVER (ORDER BY rel_ns)) / 1e9 AS interval_sec
    FROM throughput_counters
    WHERE scenario_id = ?
)
SELECT 
    rel_ns / 1e9 AS elapsed_sec,
    recv_delta / interval_sec AS msg_per_sec
FROM deltas
WHERE recv_delta IS NOT NULL;
```

---

## 7.5 JSON Export

Generate final aggregated results:

```json
{
  "scenario_id": "550e8400-e29b-41d4-a716-446655440000",
  "scenario_name": "fan-in",
  "qos": 2,
  "config": {
    "publishers": 1000,
    "subscribers": 10,
    "duration_sec": 60
  },
  "throughput": {
    "total_sent": 1200000,
    "total_received": 1198542,
    "avg_send_rate": 20000,
    "avg_receive_rate": 19975
  },
  "latency": {
    "min_ns": 89000,
    "max_ns": 145623000,
    "mean_ns": 1245000,
    "p50_ns": 892000,
    "p95_ns": 3456000,
    "p99_ns": 12345000,
    "p999_ns": 45678000,
    "histogram": {
      "buckets_ns": [100000, 250000, 500000, ...],
      "counts": [12345, 45678, 89012, ...]
    }
  },
  "delivery_rate": 0.9987
}
```

---

## 7.6 Database Lifecycle

### Single Database for Comparisons

Use **one DuckDB file** across all benchmark runs. This enables direct cross-broker and cross-configuration comparisons:

```bash
# All runs append to same database
mqtt-bench run fan-in --broker-name vibemq --db benchmarks.duckdb
mqtt-bench run fan-in --broker-name mosquitto --db benchmarks.duckdb
mqtt-bench run fan-in --broker-name emqx --db benchmarks.duckdb

# Compare directly
duckdb benchmarks.duckdb "
  SELECT broker_name, avg(receive_rate) 
  FROM scenarios s JOIN runs r ON s.run_id = r.run_id
  GROUP BY broker_name
"
```

### When to Use Separate Databases

| Scenario | Recommendation |
|----------|----------------|
| Comparing brokers | Single database |
| Comparing configs | Single database |
| CI/CD per-commit | Single database (append history) |
| Isolated experiments | Separate database |
| Sharing results | Export to Parquet |

### Parquet Export for Sharing

DuckDB files are not portable across architectures. For sharing results or loading into other tools, export to Parquet:

```bash
# Export all results to Parquet (portable, columnar, compressed)
mqtt-bench export parquet --db benchmarks.duckdb --output results/

# Creates:
#   results/runs.parquet
#   results/scenarios.parquet
#   results/latency_histograms.parquet
#   results/throughput_counters.parquet
#   results/container_samples.parquet
```

```sql
-- Or manually via DuckDB
COPY runs TO 'runs.parquet' (FORMAT PARQUET);
COPY latency_histograms TO 'latency_histograms.parquet' (FORMAT PARQUET);
```

### Parquet Benefits

| Feature | DuckDB | Parquet |
|---------|--------|---------|
| Query in-place | ✅ | ✅ (via DuckDB/Pandas/Spark) |
| Cross-platform | ❌ | ✅ |
| Append writes | ✅ | ❌ (immutable) |
| Compression | Good | Excellent |
| Share via S3/GCS | Awkward | Native |

### Recommended Workflow

```
┌─────────────────────────────────────────────────────────────┐
│                     Benchmark Runs                          │
│  mqtt-bench run ... --db benchmarks.duckdb                  │
│  mqtt-bench run ... --db benchmarks.duckdb                  │
│  mqtt-bench run ... --db benchmarks.duckdb                  │
└─────────────────────┬───────────────────────────────────────┘
                      │
                      ▼
┌─────────────────────────────────────────────────────────────┐
│              benchmarks.duckdb (local)                      │
│  • All runs accumulated                                     │
│  • Fast ad-hoc queries                                      │
│  • Comparison queries                                       │
└─────────────────────┬───────────────────────────────────────┘
                      │
          ┌───────────┴───────────┐
          ▼                       ▼
┌──────────────────┐    ┌──────────────────┐
│  results.json    │    │  *.parquet       │
│  (aggregated)    │    │  (raw, portable) │
│  • Web dashboard │    │  • S3 upload     │
│  • CI artifacts  │    │  • Team sharing  │
└──────────────────┘    │  • Jupyter/Pandas│
                        └──────────────────┘
```

### Database Location

Default location: `./mqtt-bench.duckdb` in current directory.

Override with `--db` flag or `MQTT_BENCH_DB` environment variable:

```bash
# Explicit path
mqtt-bench run fan-in --db /data/benchmarks/mqtt.duckdb

# Environment variable
export MQTT_BENCH_DB=/data/benchmarks/mqtt.duckdb
mqtt-bench run fan-in
```

---

## 7.7 Storage Comparison

| Approach | Rows/min @ 20K msg/s | Size/min |
|----------|----------------------|----------|
| Raw samples | 1,200,000 | ~30 MB |
| 1s checkpoints | 60 | ~10 KB |
| **10s checkpoints (default)** | **6** | **~1 KB** |
| 30s checkpoints | 2 | ~300 B |

**200,000x reduction in write rate vs raw samples.**

---

## 7.8 Trade-offs

### Pros
- Constant write rate regardless of throughput
- Standard Prometheus-compatible format
- Efficient aggregation queries

### Cons
- Percentile accuracy limited by bucket boundaries
- Can't compute arbitrary percentiles (e.g., P99.99 needs more buckets)
- Loses individual sample detail

### Mitigation
- Choose bucket boundaries to cover expected latency range
- Add more buckets in high-interest ranges (1-10ms)
- Store min/max per interval for outlier visibility
