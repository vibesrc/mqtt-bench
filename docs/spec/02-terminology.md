# Section 2: Terminology

## RFC 2119 Keywords

The key words "MUST", "MUST NOT", "REQUIRED", "SHALL", "SHALL NOT", "SHOULD", "SHOULD NOT", "RECOMMENDED", "MAY", and "OPTIONAL" in this document are to be interpreted as described in RFC 2119.

## Definitions

### Benchmark Terms

| Term | Definition |
|------|------------|
| **Scenario** | A specific traffic pattern configuration (fan-in, fan-out, etc.) |
| **Run** | A single execution of a scenario with fixed parameters |
| **Suite** | Multiple runs across different scenarios and QoS levels |
| **Warmup** | Initial period excluded from metrics to allow system stabilization |

### MQTT Terms

| Term | Definition |
|------|------------|
| **Publisher** | MQTT client that sends messages to topics |
| **Subscriber** | MQTT client that receives messages from topic subscriptions |
| **QoS** | Quality of Service level (0=at-most-once, 1=at-least-once, 2=exactly-once) |
| **Topic** | Hierarchical string used for message routing (e.g., `bench/topic/1`) |
| **Shared Subscription** | MQTT 5.0 feature for load-balancing messages across subscribers |

### Metrics Terms

| Term | Definition |
|------|------------|
| **End-to-end latency** | Time from publisher `publish()` call to subscriber message receipt |
| **HDR Histogram** | High Dynamic Range histogram for accurate percentile tracking |
| **P50/P95/P99/P99.9** | Latency percentiles (median, 95th, 99th, 99.9th) |
| **Throughput** | Messages processed per second (msg/s) |
| **Delivery rate** | Ratio of messages received to messages sent (0.0-1.0) |

### Container Terms

| Term | Definition |
|------|------------|
| **Container stats** | CPU, memory, network I/O metrics from Docker Engine |
| **Sampling interval** | Frequency of container stats collection (default: 1 second) |

## Abbreviations

| Abbreviation | Expansion |
|--------------|-----------|
| API | Application Programming Interface |
| CPU | Central Processing Unit |
| CSV | Comma-Separated Values |
| HDR | High Dynamic Range |
| JSON | JavaScript Object Notation |
| MQTT | Message Queuing Telemetry Transport |
| QoS | Quality of Service |
| TLS | Transport Layer Security |
