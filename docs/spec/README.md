# MQTT Broker Benchmark Suite Specification

**Version:** 1.0.0-draft  
**Status:** Draft  
**Last Updated:** 2024-12-20

## Abstract

This specification defines a high-performance MQTT broker benchmarking tool written in Rust. The tool supports multiple traffic patterns (fan-in, fan-out, straight-run, round-robin), collects end-to-end latency metrics with HDR histograms, gathers Docker container statistics via the Docker API, and outputs results in web-friendly formats (JSON, CSV) for visualization.

## Table of Contents

1. [Abstract](./00-abstract.md)
2. [Introduction](./01-introduction.md)
3. [Terminology](./02-terminology.md)
4. [Architecture](./03-architecture.md)
5. [Benchmark Scenarios](./04-scenarios.md)
6. [Metrics Collection](./05-metrics.md)
7. [Container Monitoring](./06-container-stats.md)
8. [Output Formats](./07-output.md)
9. [Implementation Notes](./08-implementation.md)
10. [References](./09-references.md)

## Document Index

| Section | File | Description | Lines |
|---------|------|-------------|-------|
| Abstract | [00-abstract.md](./00-abstract.md) | Status and summary | ~40 |
| Introduction | [01-introduction.md](./01-introduction.md) | Goals and scope | ~80 |
| Terminology | [02-terminology.md](./02-terminology.md) | Definitions and conventions | ~60 |
| Architecture | [03-architecture.md](./03-architecture.md) | System design with diagrams | ~200 |
| Scenarios | [04-scenarios.md](./04-scenarios.md) | Traffic pattern definitions | ~250 |
| Metrics | [05-metrics.md](./05-metrics.md) | Latency and throughput collection | ~150 |
| Container Stats | [06-container-stats.md](./06-container-stats.md) | Docker API integration | ~120 |
| Output | [07-output.md](./07-output.md) | JSON/CSV formats for web | ~100 |
| Implementation | [08-implementation.md](./08-implementation.md) | Rust crates and structure | ~80 |
| References | [09-references.md](./09-references.md) | External references | ~30 |

## Quick Navigation

- **Implementers**: Start with [Architecture](./03-architecture.md) and [Implementation Notes](./08-implementation.md)
- **Scenario Design**: See [Benchmark Scenarios](./04-scenarios.md) for traffic patterns
- **Metrics Details**: See [Metrics Collection](./05-metrics.md) for latency measurement
- **Output Schema**: See [Output Formats](./07-output.md) for JSON/CSV structure
