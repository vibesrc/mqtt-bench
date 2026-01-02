use super::{Scenario, ScenarioParams, TOPIC_PREFIX};
use crate::client::{PublisherConfig, SubscriberConfig};
use rumqttc::QoS;
use std::time::Duration;

/// Fan-out scenario: Few publishers, many subscribers
/// Simulates alert broadcast / market data feed
pub struct FanOutScenario {
    params: ScenarioParams,
}

impl FanOutScenario {
    pub fn new(params: ScenarioParams) -> Self {
        Self { params }
    }
}

impl Scenario for FanOutScenario {
    fn publisher_configs(
        &self,
        host: &str,
        port: u16,
        qos: QoS,
        rate: u32,
        payload_size: usize,
    ) -> Vec<PublisherConfig> {
        (0..self.params.publishers)
            .map(|i| PublisherConfig {
                client_id: format!("mqtt-bench-pub-{}", i),
                host: host.to_string(),
                port,
                // Publishers cycle through all topics
                // Topic selection happens at publish time based on message count
                // For simplicity, we'll use a fixed topic per publisher that cycles
                topic: format!("{}/{}", TOPIC_PREFIX, i % self.params.topics),
                qos,
                payload_size,
                rate,
                connect_timeout: Duration::from_secs(25),
            })
            .collect()
    }

    fn subscriber_configs(&self, host: &str, port: u16, qos: QoS) -> Vec<SubscriberConfig> {
        (0..self.params.subscribers)
            .map(|i| SubscriberConfig {
                client_id: format!("mqtt-bench-sub-{}", i),
                host: host.to_string(),
                port,
                // All subscribers subscribe to all topics using multi-level wildcard
                topic_filter: format!("{}/#", TOPIC_PREFIX),
                qos,
                connect_timeout: Duration::from_secs(25),
            })
            .collect()
    }

    fn expected_messages(&self, rate: u32, duration_secs: u64) -> u64 {
        // Each message is delivered to ALL subscribers
        // Total = publishers * rate * duration * subscribers
        (self.params.publishers as u64) * (rate as u64) * duration_secs * (self.params.subscribers as u64)
    }

    fn name(&self) -> &'static str {
        "fan-out"
    }
}
