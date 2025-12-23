use crate::db::Database;
use anyhow::Result;
use clap::ValueEnum;
use serde::Serialize;
use std::path::Path;
use uuid::Uuid;

/// Export format options
#[derive(Clone, ValueEnum)]
pub enum ExportFormat {
    /// Export to JSON
    Json,
    /// Export to Parquet files
    Parquet,
}

/// Export results in various formats
pub fn export(
    db: &Database,
    format: ExportFormat,
    output_dir: &Path,
    run_id: Option<Uuid>,
    _scenario_id: Option<Uuid>,
) -> Result<()> {
    std::fs::create_dir_all(output_dir)?;

    match format {
        ExportFormat::Json => export_json(db, output_dir, run_id),
        ExportFormat::Parquet => db.export_to_parquet(output_dir),
    }
}

fn export_json(
    db: &Database,
    output_dir: &Path,
    run_id: Option<Uuid>,
) -> Result<()> {
    if let Some(run_id) = run_id {
        if let Some(run_with_scenarios) = db.get_run_with_scenarios(run_id)? {
            let output_path = output_dir.join(format!("run-{}.json", run_id));
            let json = serde_json::to_string_pretty(&RunExport::from(run_with_scenarios))?;
            std::fs::write(&output_path, json)?;
            println!("Exported run to {}", output_path.display());
        } else {
            println!("Run {} not found", run_id);
        }
    } else {
        // Export all runs
        let runs = db.list_runs(100)?;
        for run in runs {
            if let Some(run_with_scenarios) = db.get_run_with_scenarios(run.run_id)? {
                let output_path = output_dir.join(format!("run-{}.json", run.run_id));
                let json = serde_json::to_string_pretty(&RunExport::from(run_with_scenarios))?;
                std::fs::write(&output_path, json)?;
                println!("Exported run to {}", output_path.display());
            }
        }
    }

    Ok(())
}

/// List recent benchmark runs
pub fn list_runs(db: &Database, limit: u32) -> Result<()> {
    let runs = db.list_runs(limit)?;

    if runs.is_empty() {
        println!("No benchmark runs found.");
        return Ok(());
    }

    println!();
    println!("{:<38} {:<20} {:<15} {:<20}", "RUN ID", "STARTED", "BROKER", "HOST");
    println!("{}", "─".repeat(95));

    for run in runs {
        let broker = run.broker_name.as_deref().unwrap_or("-");
        println!(
            "{:<38} {:<20} {:<15} {:<20}",
            run.run_id, run.started_at, broker, run.broker_host
        );
    }

    println!();
    Ok(())
}

/// Show detailed results for a run
pub fn show_run(db: &Database, run_id: Uuid) -> Result<()> {
    let run_with_scenarios = match db.get_run_with_scenarios(run_id)? {
        Some(r) => r,
        None => {
            println!("Run {} not found", run_id);
            return Ok(());
        }
    };

    let run = &run_with_scenarios.run;

    println!();
    println!("═══════════════════════════════════════════════════════════════");
    println!("                        RUN DETAILS                             ");
    println!("═══════════════════════════════════════════════════════════════");
    println!();
    println!("  Run ID:          {}", run.run_id);
    println!("  Started:         {}", run.started_at);
    if let Some(ended) = &run.ended_at {
        println!("  Ended:           {}", ended);
    }
    println!("  Broker:          {}:{}", run.broker_host, run.broker_port);
    if let Some(name) = &run.broker_name {
        println!("  Broker Name:     {}", name);
    }
    if let Some(version) = &run.broker_version {
        println!("  Broker Version:  {}", version);
    }
    if let Some(container) = &run.container_name {
        println!("  Container:       {}", container);
    }
    if let Some(host) = &run.hostname {
        println!("  Hostname:        {}", host);
    }
    if let Some(notes) = &run.notes {
        println!("  Notes:           {}", notes);
    }

    println!();
    println!("  SCENARIOS");
    println!("  ─────────────────────────────────────────────────────────────");

    for scenario in &run_with_scenarios.scenarios {
        println!();
        println!("  {} @ QoS {}", scenario.scenario_name, scenario.qos);
        println!("    Publishers:    {}", scenario.publishers);
        println!("    Subscribers:   {}", scenario.subscribers);
        println!("    Topics:        {}", scenario.topics);
        println!("    Rate:          {} msg/s", scenario.msg_rate);
        println!("    Payload:       {} bytes", scenario.payload_size);
        println!();
        println!("    Messages:      {} sent / {} received", scenario.total_sent, scenario.total_received);
        println!("    Avg Rate:      {:.0} msg/s", scenario.avg_recv_rate);
        println!("    Delivery:      {:.2}%", scenario.delivery_rate * 100.0);
        println!();
        println!("    Latency:");
        println!("      P50:         {}", format_latency_ns(scenario.p50_ns as u64));
        println!("      P95:         {}", format_latency_ns(scenario.p95_ns as u64));
        println!("      P99:         {}", format_latency_ns(scenario.p99_ns as u64));
        println!("      P99.9:       {}", format_latency_ns(scenario.p999_ns as u64));
    }

    println!();
    Ok(())
}

fn format_latency_ns(ns: u64) -> String {
    if ns >= 1_000_000_000 {
        format!("{:.2}s", ns as f64 / 1_000_000_000.0)
    } else if ns >= 1_000_000 {
        format!("{:.2}ms", ns as f64 / 1_000_000.0)
    } else if ns >= 1_000 {
        format!("{:.2}µs", ns as f64 / 1_000.0)
    } else {
        format!("{}ns", ns)
    }
}

/// Export format for JSON serialization
#[derive(Debug, Serialize)]
struct RunExport {
    run_id: String,
    started_at: String,
    ended_at: Option<String>,
    broker: BrokerInfo,
    scenarios: Vec<ScenarioExport>,
}

#[derive(Debug, Serialize)]
struct BrokerInfo {
    host: String,
    port: i32,
    name: Option<String>,
    version: Option<String>,
}

#[derive(Debug, Serialize)]
struct ScenarioExport {
    scenario_id: String,
    scenario_name: String,
    config: ScenarioConfig,
    throughput: ThroughputExport,
    latency: LatencyExport,
}

#[derive(Debug, Serialize)]
struct ScenarioConfig {
    qos: i32,
    publishers: i32,
    subscribers: i32,
    topics: i32,
    msg_rate: i32,
    payload_size: i32,
}

#[derive(Debug, Serialize)]
struct ThroughputExport {
    total_sent: i64,
    total_received: i64,
    avg_recv_rate: f64,
    delivery_rate: f64,
}

#[derive(Debug, Serialize)]
struct LatencyExport {
    p50_ns: i64,
    p95_ns: i64,
    p99_ns: i64,
    p999_ns: i64,
    p50_ms: f64,
    p95_ms: f64,
    p99_ms: f64,
    p999_ms: f64,
}

impl From<crate::db::RunWithScenarios> for RunExport {
    fn from(r: crate::db::RunWithScenarios) -> Self {
        RunExport {
            run_id: r.run.run_id.to_string(),
            started_at: r.run.started_at,
            ended_at: r.run.ended_at,
            broker: BrokerInfo {
                host: r.run.broker_host,
                port: r.run.broker_port,
                name: r.run.broker_name,
                version: r.run.broker_version,
            },
            scenarios: r
                .scenarios
                .into_iter()
                .map(|s| ScenarioExport {
                    scenario_id: s.scenario_id.to_string(),
                    scenario_name: s.scenario_name,
                    config: ScenarioConfig {
                        qos: s.qos,
                        publishers: s.publishers,
                        subscribers: s.subscribers,
                        topics: s.topics,
                        msg_rate: s.msg_rate,
                        payload_size: s.payload_size,
                    },
                    throughput: ThroughputExport {
                        total_sent: s.total_sent,
                        total_received: s.total_received,
                        avg_recv_rate: s.avg_recv_rate,
                        delivery_rate: s.delivery_rate,
                    },
                    latency: LatencyExport {
                        p50_ns: s.p50_ns,
                        p95_ns: s.p95_ns,
                        p99_ns: s.p99_ns,
                        p999_ns: s.p999_ns,
                        p50_ms: s.p50_ns as f64 / 1_000_000.0,
                        p95_ms: s.p95_ns as f64 / 1_000_000.0,
                        p99_ms: s.p99_ns as f64 / 1_000_000.0,
                        p999_ms: s.p999_ns as f64 / 1_000_000.0,
                    },
                })
                .collect(),
        }
    }
}
