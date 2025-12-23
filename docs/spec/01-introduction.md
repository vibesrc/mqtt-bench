# Section 1: Introduction

## Background

MQTT (Message Queuing Telemetry Transport) is the de facto standard for IoT messaging, but broker performance varies dramatically across implementations. Existing benchmark tools often measure only throughput, ignore latency percentiles, or fail to simulate realistic traffic patterns.

This specification addresses these gaps by defining a comprehensive benchmark suite that measures what matters for production deployments: sustained throughput under load, tail latency under pressure, and resource consumption over time.

## Scope

This specification covers:

- Four benchmark scenarios representing common MQTT usage patterns
- Latency measurement methodology using embedded timestamps
- Container resource monitoring via Docker Engine API
- Output formats optimized for web visualization

This specification does NOT cover:

- MQTT protocol conformance testing
- TLS/authentication overhead benchmarking (future extension)
- Multi-node broker cluster testing (future extension)

## Design Principles

1. **Accuracy over throughput** — Correct latency measurement matters more than maximizing msg/s
2. **Reproducibility** — Same configuration MUST produce comparable results across runs
3. **Minimal overhead** — Benchmark tool MUST NOT become the bottleneck
4. **Web-native output** — Results SHOULD be immediately usable in dashboards

## Document Organization

- [Section 2](./02-terminology.md) defines terms and conventions
- [Section 3](./03-architecture.md) describes overall system architecture
- [Section 4](./04-scenarios.md) specifies the four benchmark scenarios
- [Section 5](./05-metrics.md) details latency and throughput measurement
- [Section 6](./06-container-stats.md) covers Docker API integration
- [Section 7](./07-output.md) defines JSON and CSV output schemas
- [Section 8](./08-implementation.md) lists Rust crates and project structure
