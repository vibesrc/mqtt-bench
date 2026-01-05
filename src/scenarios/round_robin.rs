use super::{Scenario, ScenarioParams};
use crate::client::{PublisherConfig, SubscriberConfig};
use rumqttc::QoS;
use std::time::Duration;

/// Round-robin scenario: Shared subscriptions for load balancing
/// Requires MQTT 5.0 broker support
/// Simulates work queue distribution
pub struct RoundRobinScenario {
    params: ScenarioParams,
}

impl RoundRobinScenario {
    pub fn new(params: ScenarioParams) -> Self {
        Self { params }
    }
}

impl Scenario for RoundRobinScenario {
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
                // All publishers cycle through all topics
                topic: format!("{}/{}", base_topic, i % self.params.topics),
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
                // All subscribers use shared subscription for load balancing
                // $share/{ShareGroup}/{TopicFilter}
                topic_filter: format!("$share/benchgroup/{}/#", base_topic),
                qos,
                connect_timeout: Duration::from_secs(25),
            })
            .collect()
    }

    fn expected_messages(&self, rate: u32, duration_secs: u64) -> u64 {
        // Each message delivered to exactly ONE subscriber (load balanced)
        (self.params.publishers as u64) * (rate as u64) * duration_secs
    }

    fn name(&self) -> &'static str {
        "round-robin"
    }
}
