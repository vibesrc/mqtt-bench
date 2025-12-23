# Section 6: Container Monitoring

## Overview

To understand broker resource consumption during benchmarks, the tool MUST collect container statistics via the Docker Engine API. This provides CPU, memory, and network metrics correlated with benchmark execution.

## 6.1 Docker Engine API

### Connection

The tool MUST connect to the Docker Engine API via:

1. **Unix socket** (default): `/var/run/docker.sock`
2. **TCP**: `http://localhost:2375` or `https://localhost:2376`

Connection method SHOULD be configurable via CLI or environment variable.

### Authentication

For remote Docker daemons with TLS:

- Client certificate authentication SHOULD be supported
- Environment variables: `DOCKER_HOST`, `DOCKER_CERT_PATH`, `DOCKER_TLS_VERIFY`

---

## 6.2 Stats Endpoint

### API Call

Container statistics are retrieved via:

```
GET /containers/{container_id}/stats?stream=false
```

The `stream=false` parameter returns a single stats snapshot rather than a continuous stream.

### Response Structure

The relevant fields from the Docker stats response:

```
{
  "read": "2024-12-20T10:30:00.000000000Z",
  "cpu_stats": {
    "cpu_usage": {
      "total_usage": 123456789000,        // nanoseconds
      "usage_in_kernelmode": 12345678900,
      "usage_in_usermode": 111111110100
    },
    "system_cpu_usage": 9876543210000000,
    "online_cpus": 4
  },
  "precpu_stats": { ... },                 // previous sample for delta
  "memory_stats": {
    "usage": 104857600,                    // bytes
    "max_usage": 209715200,
    "limit": 536870912                     // container memory limit
  },
  "networks": {
    "eth0": {
      "rx_bytes": 1048576,
      "rx_packets": 1024,
      "tx_bytes": 2097152,
      "tx_packets": 2048
    }
  }
}
```

---

## 6.3 Calculated Metrics

### CPU Usage Percentage

CPU percentage MUST be calculated as:

```
cpu_delta = cpu_stats.cpu_usage.total_usage - precpu_stats.cpu_usage.total_usage
system_delta = cpu_stats.system_cpu_usage - precpu_stats.system_cpu_usage
cpu_percent = (cpu_delta / system_delta) × online_cpus × 100
```

### Memory Usage

Memory metrics:

| Metric | Calculation | Unit |
|--------|-------------|------|
| `memory_usage_bytes` | `memory_stats.usage` | bytes |
| `memory_limit_bytes` | `memory_stats.limit` | bytes |
| `memory_percent` | `usage / limit × 100` | percent |

### Network I/O

Network metrics (summed across all interfaces):

| Metric | Source | Unit |
|--------|--------|------|
| `network_rx_bytes` | `networks.*.rx_bytes` | bytes |
| `network_tx_bytes` | `networks.*.tx_bytes` | bytes |
| `network_rx_packets` | `networks.*.rx_packets` | count |
| `network_tx_packets` | `networks.*.tx_packets` | count |

---

## 6.4 Collection Strategy

### Polling Interval

Container stats MUST be collected at a configurable interval:

| Parameter | Default | Range |
|-----------|---------|-------|
| `stats_interval_ms` | 1000 | 100 - 10000 |

### Collection Timeline

```
┌─────────┬─────────────────────────────────────────┬─────────┐
│ Warmup  │              Measurement                │ Cooldown│
├─────────┼─────────────────────────────────────────┼─────────┤
│    5s   │                  60s                    │   5s    │
└─────────┴─────────────────────────────────────────┴─────────┘
     ▲         ▲    ▲    ▲    ▲    ▲    ▲    ▲          ▲
     │         │    │    │    │    │    │    │          │
   Stats     Stats collection every 1 second        Final
   start     (60 samples during measurement)        stats
```

*Figure 6-1: Container stats collection timeline*

### Data Points

Each stats sample produces a record:

| Field | Type | Description |
|-------|------|-------------|
| `timestamp` | ISO 8601 | Sample time |
| `elapsed_seconds` | float | Seconds since benchmark start |
| `phase` | string | "warmup", "measurement", or "cooldown" |
| `cpu_percent` | float | CPU utilization percentage |
| `memory_usage_bytes` | u64 | Current memory usage |
| `memory_percent` | float | Memory utilization percentage |
| `network_rx_bytes` | u64 | Cumulative bytes received |
| `network_tx_bytes` | u64 | Cumulative bytes transmitted |

---

## 6.5 Container Identification

### CLI Parameter

The container MUST be specified via CLI:

```bash
mqtt-bench run fan-in --container broker-container-name
mqtt-bench run fan-in --container abc123def456  # container ID
```

### Auto-Detection

If `--container` is not specified but `--host` resolves to localhost, the tool MAY attempt to auto-detect the broker container by:

1. Listing containers with exposed port matching `--port`
2. Prompting user to select if multiple matches found

### No Container Mode

If no container is specified or detected, container stats collection MUST be skipped. The tool SHOULD emit a warning:

```
⚠️  No container specified, skipping resource monitoring
```

---

## 6.6 Error Handling

### Connection Failures

If Docker API connection fails:

- Log error with details
- Continue benchmark without container stats
- Note absence in output metadata

### Stats Collection Failures

If individual stats calls fail:

- Log warning
- Record null/missing values for that sample
- Continue collection attempts

### Permission Errors

Common permission issues:

| Error | Cause | Solution |
|-------|-------|----------|
| `permission denied` | User not in docker group | Add user to docker group or use sudo |
| `connection refused` | Docker daemon not running | Start Docker daemon |
| `no such container` | Container not found | Verify container name/ID |
