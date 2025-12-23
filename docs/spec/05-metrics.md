# Section 5: Metrics Collection

## Overview

Accurate metrics are the foundation of meaningful benchmarks. This section specifies how latency, throughput, and delivery metrics MUST be collected.

## 5.1 Latency Measurement

### Embedded Timestamp Method

End-to-end latency MUST be measured using embedded timestamps in message payloads:

```
┌─────────────────────────────────────────────────────────────┐
│                     Message Payload                          │
├─────────────┬───────────────────────────────────────────────┤
│  Bytes 0-7  │              Bytes 8-N                        │
│  Timestamp  │           User Payload                        │
│  (u64 ns)   │          (configurable)                       │
└─────────────┴───────────────────────────────────────────────┘
```

*Figure 5-1: Payload structure with embedded timestamp*

### Timestamp Generation

The publisher MUST:

1. Capture current time as nanoseconds since Unix epoch
2. Encode timestamp as little-endian u64 in first 8 bytes of payload
3. Fill remaining bytes with configurable padding (default: random)

### Latency Calculation

The subscriber MUST:

1. Record receive time immediately upon message delivery callback
2. Extract timestamp from first 8 bytes of payload
3. Calculate latency: `receive_time_ns - embedded_timestamp_ns`
4. Record latency in histogram (clamping negative values to 0)

> **Note:** Clock skew between publisher and subscriber machines will affect accuracy. For precise measurements, run benchmark and broker on the same host or use NTP synchronization.

### Why Not Broker Timestamps?

Some brokers add timestamps to messages. This specification rejects broker timestamps because:

- Not all brokers support this feature
- Broker timestamp measures broker processing time only, not end-to-end
- Embedded timestamps measure what applications actually experience

---

## 5.2 HDR Histogram

### Specification

Latency values MUST be recorded in an HDR (High Dynamic Range) histogram with these parameters:

| Parameter | Value | Rationale |
|-----------|-------|-----------|
| Lowest trackable value | 1 µs | Sub-microsecond precision unnecessary for network I/O |
| Highest trackable value | 10 seconds | Captures extreme outliers without overflow |
| Significant figures | 3 | 0.1% precision, reasonable memory usage |

### Recorded Percentiles

The following percentiles MUST be calculated from the histogram:

| Percentile | Field Name | Description |
|------------|------------|-------------|
| Minimum | `latency_min_us` | Lowest recorded latency |
| Maximum | `latency_max_us` | Highest recorded latency |
| Mean | `latency_mean_us` | Arithmetic mean |
| P50 | `latency_p50_us` | Median latency |
| P95 | `latency_p95_us` | 95th percentile |
| P99 | `latency_p99_us` | 99th percentile |
| P99.9 | `latency_p999_us` | 99.9th percentile |

### Thread Safety

The histogram MUST be protected for concurrent writes from multiple subscriber tasks. Implementations SHOULD use a fast mutex (e.g., `parking_lot::Mutex`) rather than the standard library mutex.

---

## 5.3 Throughput Metrics

### Counters

The following counters MUST be maintained using atomic operations:

| Counter | Description |
|---------|-------------|
| `messages_sent` | Total PUBLISH calls by publishers |
| `messages_received` | Total messages delivered to subscribers |
| `messages_acked` | Total acknowledgments received (QoS 1: PUBACK, QoS 2: PUBCOMP) |
| `bytes_sent` | Total payload bytes published |
| `bytes_received` | Total payload bytes received |
| `errors` | Connection errors, publish failures, etc. |

### Rate Calculation

Throughput rates MUST be calculated over the measurement period only (excluding warmup):

```
send_rate = messages_sent / duration_seconds
receive_rate = messages_received / duration_seconds
```

### Delivery Rate

Delivery rate measures message loss:

```
delivery_rate = messages_received / expected_messages
```

Where `expected_messages` depends on the scenario:

| Scenario | Expected Messages |
|----------|-------------------|
| Fan-In | `publishers × rate × duration` (total, divided among subscribers) |
| Fan-Out | `publishers × rate × duration × subscribers` (each msg to all subs) |
| Straight-Run | `publishers × rate × duration` (1:1 mapping) |
| Round-Robin | `publishers × rate × duration` (each msg to one sub) |

---

## 5.4 Warmup Period

### Purpose

The warmup period allows the system to stabilize before measurement:

- TCP connections fully established
- Broker internal caches populated  
- JIT compilation completed (for JVM-based brokers)
- OS network buffers sized appropriately

### Behavior

During warmup:

- Publishers and subscribers operate normally
- All metrics MUST be discarded or reset
- Container stats collection SHOULD still run (for baseline)

### Configuration

| Parameter | Default | Description |
|-----------|---------|-------------|
| `warmup_seconds` | 5 | Duration of warmup period |

The warmup period SHOULD be at least 5 seconds. For high-throughput benchmarks, 10-30 seconds is RECOMMENDED.

---

## 5.5 Sampling and Aggregation

### Real-Time Progress

During execution, the tool SHOULD display real-time progress:

```
fan-in @ QoS 2 ━━━━━━━━━━━━━━━━━━━━ 45s/60s  12,450 msg/s  P99: 23ms
```

Progress indicators SHOULD show:

- Elapsed time / total duration
- Current throughput (msg/s)
- Current P99 latency

### Final Aggregation

At benchmark completion:

1. Stop all publishers and subscribers
2. Wait for in-flight messages to complete (timeout: 5 seconds)
3. Calculate final percentiles from histogram
4. Calculate throughput rates from counters
5. Output results in configured format(s)
