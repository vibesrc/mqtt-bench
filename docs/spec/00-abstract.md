# Section 0: Abstract

## Status of This Document

This document specifies an MQTT broker benchmarking tool designed for performance testing and comparison. This is a draft specification subject to change.

## Abstract

Modern MQTT brokers claim high throughput and low latency, but meaningful comparison requires consistent, reproducible benchmarks across multiple traffic patterns. This specification defines `mqtt-bench`, a Rust-based benchmarking suite that:

1. **Simulates realistic traffic patterns** — Fan-in (IoT ingestion), fan-out (broadcast), point-to-point, and shared subscriptions
2. **Measures true end-to-end latency** — Embeds nanosecond timestamps in message payloads
3. **Collects HDR histogram percentiles** — P50, P95, P99, P99.9 with microsecond precision
4. **Monitors broker resource usage** — CPU, memory, network via Docker API
5. **Outputs web-friendly formats** — JSON and CSV for dashboards and visualization

## Goals

- Provide a single tool that benchmarks any MQTT 3.1.1/5.0 broker
- Support QoS 0, 1, and 2 with proper protocol compliance verification
- Enable automated performance regression testing in CI/CD
- Generate data suitable for web-based visualization (Grafana, custom dashboards)

## Non-Goals

- This tool does not verify MQTT protocol conformance (use mqtt-test-suite for that)
- This tool does not provide real-time monitoring (it runs discrete benchmark sessions)
- This tool does not manage broker deployment (use Docker Compose or Kubernetes)
