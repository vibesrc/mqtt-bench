pub mod fan_in;
pub mod fan_out;
pub mod round_robin;
pub mod straight_run;

use crate::client::{PublisherConfig, SubscriberConfig};
use crate::ScenarioType;
use rumqttc::QoS;

/// Trait for benchmark scenarios
#[allow(dead_code)]
pub trait Scenario {
    /// Get publisher configurations for this scenario
    fn publisher_configs(
        &self,
        host: &str,
        port: u16,
        qos: QoS,
        rate: u32,
        payload_size: usize,
        client_prefix: &str,
        base_topic: &str,
    ) -> Vec<PublisherConfig>;

    /// Get subscriber configurations for this scenario
    fn subscriber_configs(&self, host: &str, port: u16, qos: QoS, client_prefix: &str, base_topic: &str) -> Vec<SubscriberConfig>;

    /// Calculate expected messages for delivery rate calculation
    fn expected_messages(&self, rate: u32, duration_secs: u64) -> u64;

    /// Get scenario name
    fn name(&self) -> &'static str;
}

/// Scenario parameters
#[derive(Debug, Clone)]
pub struct ScenarioParams {
    pub publishers: u32,
    pub subscribers: u32,
    pub topics: u32,
}

impl ScenarioParams {
    /// Create parameters with scenario-specific defaults
    pub fn new(
        scenario: &ScenarioType,
        publishers: Option<u32>,
        subscribers: Option<u32>,
        topics: Option<u32>,
    ) -> Self {
        match scenario {
            ScenarioType::FanIn => Self {
                publishers: publishers.unwrap_or(1000),
                subscribers: subscribers.unwrap_or(10),
                topics: topics.unwrap_or(100),
            },
            ScenarioType::FanOut => Self {
                publishers: publishers.unwrap_or(10),
                subscribers: subscribers.unwrap_or(1000),
                topics: topics.unwrap_or(10),
            },
            ScenarioType::StraightRun => {
                let count = publishers.or(subscribers).or(topics).unwrap_or(100);
                Self {
                    publishers: count,
                    subscribers: count,
                    topics: count,
                }
            }
            ScenarioType::RoundRobin => Self {
                publishers: publishers.unwrap_or(100),
                subscribers: subscribers.unwrap_or(100),
                topics: topics.unwrap_or(10),
            },
        }
    }
}

/// Create a scenario instance from type
pub fn create_scenario(scenario_type: &ScenarioType, params: ScenarioParams) -> Box<dyn Scenario + Send + Sync> {
    match scenario_type {
        ScenarioType::FanIn => Box::new(fan_in::FanInScenario::new(params)),
        ScenarioType::FanOut => Box::new(fan_out::FanOutScenario::new(params)),
        ScenarioType::StraightRun => Box::new(straight_run::StraightRunScenario::new(params)),
        ScenarioType::RoundRobin => Box::new(round_robin::RoundRobinScenario::new(params)),
    }
}
