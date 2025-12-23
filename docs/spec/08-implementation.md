# Section 8: Implementation Notes

## Language

This tool MUST be implemented in **Rust** for maximum performance and minimal runtime overhead.

## 8.1 Required Crates

### Core Runtime

| Crate | Version | Purpose |
|-------|---------|---------|
| `tokio` | 1.x | Async runtime with multi-threaded scheduler |
| `tokio-util` | 0.7.x | Async utilities (codec, compat) |

### MQTT Client

| Crate | Version | Purpose |
|-------|---------|---------|
| `rumqttc` | 0.24.x | Pure Rust async MQTT 3.1.1/5.0 client |

> **Note:** `rumqttc` is preferred over `paho-mqtt` for pure Rust implementation without C dependencies.

### Metrics & Histograms

| Crate | Version | Purpose |
|-------|---------|---------|
| `hdrhistogram` | 7.x | HDR histogram for latency percentiles |
| `parking_lot` | 0.12.x | Fast mutex for histogram access |

### Concurrency

| Crate | Version | Purpose |
|-------|---------|---------|
| `crossbeam-channel` | 0.5.x | Multi-producer channels for metrics |
| `dashmap` | 5.x | Concurrent HashMap (if needed) |

### CLI & Output

| Crate | Version | Purpose |
|-------|---------|---------|
| `clap` | 4.x | Command-line argument parsing |
| `indicatif` | 0.17.x | Progress bars and spinners |
| `serde` | 1.x | Serialization framework |
| `serde_json` | 1.x | JSON output |
| `csv` | 1.x | CSV output |

### Docker API

| Crate | Version | Purpose |
|-------|---------|---------|
| `bollard` | 0.16.x | Docker Engine API client |

### Database

| Crate | Version | Purpose |
|-------|---------|---------|
| `duckdb` | 1.x | Embedded analytics database (with `bundled` feature) |

### Error Handling

| Crate | Version | Purpose |
|-------|---------|---------|
| `anyhow` | 1.x | Application error handling |
| `thiserror` | 1.x | Custom error types |

### Logging

| Crate | Version | Purpose |
|-------|---------|---------|
| `tracing` | 0.1.x | Structured logging |
| `tracing-subscriber` | 0.3.x | Log output formatting |

---

## 8.2 Project Structure

```
mqtt-bench/
├── Cargo.toml
├── Cargo.lock
├── README.md
├── src/
│   ├── main.rs           # CLI entry point, argument parsing
│   ├── lib.rs            # Library exports (optional)
│   ├── bench.rs          # Benchmark orchestration
│   ├── client.rs         # MQTT publisher/subscriber implementations
│   ├── metrics.rs        # HDR histogram and counter management
│   ├── scenarios/
│   │   ├── mod.rs        # Scenario trait definition
│   │   ├── fan_in.rs     # Fan-in implementation
│   │   ├── fan_out.rs    # Fan-out implementation
│   │   ├── straight.rs   # Straight-run implementation
│   │   └── round_robin.rs# Round-robin implementation
│   ├── docker.rs         # Docker API client wrapper
│   └── output.rs         # JSON/CSV serialization
└── tests/
    └── integration.rs    # Integration tests
```

---

## 8.3 Build Configuration

### Cargo.toml Profile

```toml
[profile.release]
opt-level = 3
lto = "thin"
codegen-units = 1
panic = "abort"
strip = true
```

### Expected Binary Size

With release optimizations: 3-5 MB (statically linked)

### Build Command

```bash
cargo build --release
```

---

## 8.4 Key Implementation Details

### Timestamp Encoding

```rust
// Publisher: embed timestamp
let timestamp = std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .unwrap()
    .as_nanos() as u64;
payload[0..8].copy_from_slice(&timestamp.to_le_bytes());

// Subscriber: extract and calculate latency
let embedded = u64::from_le_bytes(payload[0..8].try_into().unwrap());
let now = /* current nanos */;
let latency_ns = now.saturating_sub(embedded);
```

### Atomic Counters

```rust
use std::sync::atomic::{AtomicU64, Ordering};

struct Counters {
    messages_sent: AtomicU64,
    messages_received: AtomicU64,
    // ...
}

// Increment (lock-free)
counters.messages_sent.fetch_add(1, Ordering::Relaxed);

// Read snapshot
let sent = counters.messages_sent.load(Ordering::Acquire);
```

### Docker Stats Collection

```rust
use bollard::Docker;
use bollard::container::StatsOptions;

let docker = Docker::connect_with_socket_defaults()?;
let stats = docker.stats(&container_id, Some(StatsOptions {
    stream: false,
    one_shot: true,
})).next().await?;
```

---

## 8.5 Performance Considerations

### Connection Pooling

Each publisher/subscriber maintains its own connection. Do NOT share connections across tasks.

### Buffer Sizes

Configure rumqttc with appropriate buffer sizes:

```rust
let mut options = MqttOptions::new(client_id, host, port);
options.set_max_packet_size(256 * 1024, 256 * 1024);
options.set_pending_throttle(Duration::from_millis(1));
```

### Rate Limiting

Publishers SHOULD use token bucket or leaky bucket for smooth rate limiting rather than burst-then-sleep patterns.

### Memory Allocation

Pre-allocate payload buffers. Avoid allocations in the hot path.

---

## 8.6 Testing

### Unit Tests

- Scenario topic assignment logic
- Latency calculation
- Metrics aggregation

### Integration Tests

- Connect to test broker (Mosquitto in Docker)
- Run each scenario for 5 seconds
- Verify message delivery

### Benchmark Tests

```rust
#[bench]
fn bench_timestamp_encoding(b: &mut Bencher) {
    b.iter(|| {
        // measure encoding overhead
    });
}
```
