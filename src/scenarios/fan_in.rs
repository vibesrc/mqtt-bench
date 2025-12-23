use super::{Scenario, ScenarioParams, TOPIC_PREFIX};
use crate::client::{PublisherConfig, SubscriberConfig};
use rumqttc::QoS;

/// Fan-in scenario: Many publishers, few subscribers
/// Simulates IoT sensor ingestion where many devices publish to grouped topics
/// and each subscriber handles a partition of the topic space.
///
/// Topic structure: bench/group-{N}/sensor-{M}
/// - Publishers are distributed across groups round-robin
/// - Each subscriber subscribes to one group: bench/group-{N}/+
/// - Result: many-to-one within each group (true fan-in)
pub struct FanInScenario {
    params: ScenarioParams,
}

impl FanInScenario {
    pub fn new(params: ScenarioParams) -> Self {
        Self { params }
    }
}

impl Scenario for FanInScenario {
    fn publisher_configs(
        &self,
        host: &str,
        port: u16,
        qos: QoS,
        rate: u32,
        payload_size: usize,
    ) -> Vec<PublisherConfig> {
        // Number of groups = number of subscribers (each subscriber handles one group)
        let num_groups = self.params.subscribers;

        (0..self.params.publishers)
            .map(|i| {
                // Distribute publishers across groups round-robin
                let group_id = i % num_groups;
                PublisherConfig {
                    client_id: format!("mqtt-bench-pub-{}", i),
                    host: host.to_string(),
                    port,
                    topic: format!("{}/group-{}/sensor-{}", TOPIC_PREFIX, group_id, i),
                    qos,
                    payload_size,
                    rate,
                }
            })
            .collect()
    }

    fn subscriber_configs(&self, host: &str, port: u16, qos: QoS) -> Vec<SubscriberConfig> {
        (0..self.params.subscribers)
            .map(|i| SubscriberConfig {
                client_id: format!("mqtt-bench-sub-{}", i),
                host: host.to_string(),
                port,
                // Subscribe to this subscriber's group using single-level wildcard
                topic_filter: format!("{}/group-{}/+", TOPIC_PREFIX, i),
                qos,
            })
            .collect()
    }

    fn expected_messages(&self, rate: u32, duration_secs: u64) -> u64 {
        // Each message goes to exactly one subscriber (true fan-in)
        // Total messages = publishers * rate * duration
        (self.params.publishers as u64) * (rate as u64) * duration_secs
    }

    fn name(&self) -> &'static str {
        "fan-in"
    }
}
