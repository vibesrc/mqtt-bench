use super::{Scenario, ScenarioParams};
use crate::client::{PublisherConfig, SubscriberConfig};
use rumqttc::QoS;
use std::time::Duration;

/// Straight-run scenario: Equal publishers/subscribers with 1:1 topic mapping
/// Simulates point-to-point messaging
pub struct StraightRunScenario {
    params: ScenarioParams,
}

impl StraightRunScenario {
    pub fn new(params: ScenarioParams) -> Self {
        Self { params }
    }
}

impl Scenario for StraightRunScenario {
    fn publisher_configs(
        &self,
        host: &str,
        port: u16,
        qos: QoS,
        rate: u32,
        payload_size: usize,
        client_prefix: &str,
        base_topic: &str,
    ) -> Vec<PublisherConfig> {
        (0..self.params.publishers)
            .map(|i| PublisherConfig {
                client_id: format!("{}-pub-{}", client_prefix, i),
                host: host.to_string(),
                port,
                // Publisher N publishes exclusively to topic N
                topic: format!("{}/{}", base_topic, i),
                qos,
                payload_size,
                rate,
                connect_timeout: Duration::from_secs(25),
            })
            .collect()
    }

    fn subscriber_configs(&self, host: &str, port: u16, qos: QoS, client_prefix: &str, base_topic: &str) -> Vec<SubscriberConfig> {
        (0..self.params.subscribers)
            .map(|i| SubscriberConfig {
                client_id: format!("{}-sub-{}", client_prefix, i),
                host: host.to_string(),
                port,
                // Subscriber N subscribes exclusively to topic N
                topic_filter: format!("{}/{}", base_topic, i),
                qos,
                connect_timeout: Duration::from_secs(25),
            })
            .collect()
    }

    fn expected_messages(&self, rate: u32, duration_secs: u64) -> u64 {
        // 1:1 mapping, each message goes to exactly one subscriber
        (self.params.publishers as u64) * (rate as u64) * duration_secs
    }

    fn name(&self) -> &'static str {
        "straight-run"
    }
}
