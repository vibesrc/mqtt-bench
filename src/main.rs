mod bench;
mod client;
mod db;
mod docker;
mod metrics;
mod output;
mod process;
mod scenarios;
mod serve;

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use output::ExportFormat;
use std::path::PathBuf;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};
use uuid::Uuid;

#[derive(Parser)]
#[command(name = "mqtt-bench")]
#[command(about = "MQTT broker benchmarking tool", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Database file path
    #[arg(long, env = "MQTT_BENCH_DB", default_value = "mqtt-bench.duckdb")]
    db: PathBuf,

    /// Enable verbose logging
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8,
}

#[derive(Subcommand)]
enum Commands {
    /// Run a benchmark scenario
    Run {
        /// Benchmark scenario to run
        #[arg(value_enum)]
        scenario: ScenarioType,

        /// MQTT broker host
        #[arg(long, default_value = "localhost")]
        host: String,

        /// MQTT broker port
        #[arg(long, default_value = "1883")]
        port: u16,

        /// QoS level (0, 1, or 2)
        #[arg(long, default_value = "0")]
        qos: u8,

        /// Number of publishers
        #[arg(long)]
        publishers: Option<u32>,

        /// Number of subscribers
        #[arg(long)]
        subscribers: Option<u32>,

        /// Number of topics
        #[arg(long)]
        topics: Option<u32>,

        /// Message rate per publisher (msg/s)
        #[arg(long, default_value = "1")]
        rate: u32,

        /// Payload size in bytes
        #[arg(long, default_value = "64")]
        payload_size: u32,

        /// Warmup duration
        #[arg(long, default_value = "5s")]
        warmup: humantime::Duration,

        /// Benchmark duration
        #[arg(long, default_value = "60s")]
        duration: humantime::Duration,

        /// Docker container name or ID for resource monitoring
        #[arg(long)]
        container: Option<String>,

        /// Broker name (for labeling results)
        #[arg(long)]
        broker_name: Option<String>,

        /// Broker version (for labeling results)
        #[arg(long)]
        broker_version: Option<String>,

        /// Checkpoint interval for metrics aggregation
        #[arg(long, default_value = "1s")]
        checkpoint_interval: humantime::Duration,

        /// Docker stats collection interval (shorter = more accurate CPU/network stats)
        #[arg(long, default_value = "1s")]
        stats_interval: humantime::Duration,

        /// Message timeout - messages taking longer are considered lost
        #[arg(long, default_value = "10s")]
        timeout: humantime::Duration,

        /// Notes for this benchmark run
        #[arg(long)]
        notes: Option<String>,

        /// Client ID prefix (default: mqtt-bench)
        #[arg(long, default_value = "mqtt-bench")]
        client_prefix: String,

        /// Base topic prefix for MQTT messages (default: bench/topic)
        #[arg(long, default_value = "bench/topic")]
        base_topic: String,
    },

    /// Export benchmark results
    Export {
        /// Export format
        #[arg(value_enum)]
        format: ExportFormat,

        /// Output directory
        #[arg(long, default_value = ".")]
        output: PathBuf,

        /// Filter by run ID
        #[arg(long)]
        run_id: Option<uuid::Uuid>,

        /// Filter by scenario ID
        #[arg(long)]
        scenario_id: Option<uuid::Uuid>,
    },

    /// List benchmark runs
    List {
        /// Number of runs to show
        #[arg(long, default_value = "10")]
        limit: u32,
    },

    /// Show detailed results for a run
    Show {
        /// Run ID to show
        run_id: uuid::Uuid,
    },

    /// Start the web viewer server
    Serve {
        /// Port to serve on
        #[arg(long, short, default_value = "8080")]
        port: u16,

        /// Open browser automatically
        #[arg(long, short)]
        open: bool,
    },
}

#[derive(Clone, ValueEnum)]
pub enum ScenarioType {
    /// Many publishers, few subscribers (IoT ingestion)
    FanIn,
    /// Few publishers, many subscribers (broadcast)
    FanOut,
    /// Equal publishers/subscribers with 1:1 topic mapping
    StraightRun,
    /// Shared subscriptions for load balancing (MQTT 5.0)
    RoundRobin,
}

impl std::fmt::Display for ScenarioType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScenarioType::FanIn => write!(f, "fan-in"),
            ScenarioType::FanOut => write!(f, "fan-out"),
            ScenarioType::StraightRun => write!(f, "straight-run"),
            ScenarioType::RoundRobin => write!(f, "round-robin"),
        }
    }
}


#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Determine console filter based on verbosity
    let console_filter = match cli.verbose {
        0 => "mqtt_bench=info",
        1 => "mqtt_bench=debug",
        _ => "mqtt_bench=trace,rumqttc=debug",
    };

    // Check if this is a Run command - if so, set up file logging
    let run_id = if matches!(cli.command, Commands::Run { .. }) {
        Some(Uuid::new_v4())
    } else {
        None
    };

    // Initialize tracing with optional file logging
    if let Some(rid) = run_id {
        let log_file = std::fs::File::create(format!("{}.log", rid))?;

        // Console layer: respects verbosity
        let console_layer = fmt::layer()
            .with_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(console_filter)));

        // File layer: always debug level
        let file_layer = fmt::layer()
            .with_writer(log_file)
            .with_ansi(false)
            .with_filter(EnvFilter::new("mqtt_bench=debug"));

        tracing_subscriber::registry()
            .with(console_layer)
            .with(file_layer)
            .init();
    } else {
        tracing_subscriber::registry()
            .with(fmt::layer())
            .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(console_filter)))
            .init();
    }

    // Initialize database
    let db = db::Database::open(&cli.db)?;
    db.init_schema()?;

    match cli.command {
        Commands::Run {
            scenario,
            host,
            port,
            qos,
            publishers,
            subscribers,
            topics,
            rate,
            payload_size,
            warmup,
            duration,
            container,
            broker_name,
            broker_version,
            checkpoint_interval,
            stats_interval,
            timeout,
            notes,
            client_prefix,
            base_topic,
        } => {
            let config = bench::BenchmarkConfig {
                run_id: run_id.expect("run_id should be set for Run command"),
                scenario,
                host,
                port,
                qos,
                publishers,
                subscribers,
                topics,
                rate,
                payload_size,
                warmup: warmup.into(),
                duration: duration.into(),
                container,
                broker_name,
                broker_version,
                checkpoint_interval: checkpoint_interval.into(),
                stats_interval: stats_interval.into(),
                timeout: timeout.into(),
                notes,
                client_prefix,
                base_topic,
            };

            let orchestrator = bench::Orchestrator::new(db, config);
            orchestrator.run().await?;
        }

        Commands::Export {
            format,
            output,
            run_id,
            scenario_id,
        } => {
            output::export(&db, format, &output, run_id, scenario_id)?;
        }

        Commands::List { limit } => {
            output::list_runs(&db, limit)?;
        }

        Commands::Show { run_id } => {
            output::show_run(&db, run_id)?;
        }

        Commands::Serve { port, open } => {
            // For serve, we don't need to open the database with DuckDB
            // Just pass the path to serve the file
            drop(db); // Close the database connection

            let db_path = if cli.db.exists() {
                Some(cli.db)
            } else {
                None
            };

            let config = serve::ServeConfig {
                port,
                db_path,
                open_browser: open,
            };

            serve::serve(config).await?;
        }
    }

    Ok(())
}
