# Section 9: References

## Normative References

| Reference | Title | URL |
|-----------|-------|-----|
| RFC 2119 | Key words for use in RFCs | https://www.rfc-editor.org/rfc/rfc2119 |
| MQTT 3.1.1 | OASIS Standard | https://docs.oasis-open.org/mqtt/mqtt/v3.1.1/mqtt-v3.1.1.html |
| MQTT 5.0 | OASIS Standard | https://docs.oasis-open.org/mqtt/mqtt/v5.0/mqtt-v5.0.html |
| Docker Engine API | v1.43 | https://docs.docker.com/engine/api/v1.43/ |

## Informative References

### Rust Crates

| Crate | Repository |
|-------|------------|
| tokio | https://github.com/tokio-rs/tokio |
| rumqttc | https://github.com/bytebeamio/rumqtt |
| hdrhistogram | https://github.com/HdrHistogram/HdrHistogram_rust |
| bollard | https://github.com/fussybeaver/bollard |
| clap | https://github.com/clap-rs/clap |
| indicatif | https://github.com/console-rs/indicatif |

### HDR Histogram

| Reference | URL |
|-----------|-----|
| HDR Histogram Overview | https://hdrhistogram.github.io/HdrHistogram/ |
| How NOT to Measure Latency | https://www.youtube.com/watch?v=lJ8ydIuPFeU |

### MQTT Benchmarking

| Reference | Description |
|-----------|-------------|
| emqtt-bench | EMQX benchmark tool | https://github.com/emqx/emqtt-bench |
| mqttloader | MQTT load testing tool | https://github.com/dist-sys/mqttloader |

## Acknowledgments

This specification draws inspiration from:

- Gil Tene's work on latency measurement and HDR Histogram
- The EMQX team's approach to MQTT benchmarking
- The Rust async ecosystem, particularly Tokio

## Revision History

| Version | Date | Changes |
|---------|------|---------|
| 1.0.0-draft | 2024-12-20 | Initial draft |
