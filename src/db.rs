use crate::docker::ContainerSample;
use crate::metrics::{
    CounterSnapshot, ExactPercentiles, HistogramSnapshot, BUCKET_NAMES, NUM_BUCKETS,
};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use duckdb::{params, Connection};
use std::path::Path;
use std::sync::Mutex;
use tracing::{debug, info};
use uuid::Uuid;

/// Database wrapper for benchmark results storage
pub struct Database {
    conn: Mutex<Connection>,
}

impl Database {
    /// Open or create a database file
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path).context("Failed to open database")?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Initialize the database schema
    pub fn init_schema(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();

        // Create runs table
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS runs (
                run_id          UUID PRIMARY KEY,
                started_at      TIMESTAMP NOT NULL,
                ended_at        TIMESTAMP,

                broker_host     VARCHAR NOT NULL,
                broker_port     INTEGER NOT NULL,
                broker_name     VARCHAR,
                broker_version  VARCHAR,

                container_id    VARCHAR,
                container_name  VARCHAR,

                hostname        VARCHAR,
                cpus            INTEGER,
                memory_bytes    BIGINT,

                git_commit      VARCHAR,
                notes           VARCHAR
            );
            "#,
        )?;

        // Create scenarios table
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS scenarios (
                scenario_id     UUID PRIMARY KEY,
                run_id          UUID,
                started_at      TIMESTAMP NOT NULL,
                ended_at        TIMESTAMP,

                scenario_name   VARCHAR NOT NULL,
                qos             INTEGER NOT NULL,
                publishers      INTEGER NOT NULL,
                subscribers     INTEGER NOT NULL,
                topics          INTEGER NOT NULL,
                msg_rate        INTEGER NOT NULL,
                payload_size    INTEGER NOT NULL,
                warmup_ns       BIGINT NOT NULL,
                duration_ns     BIGINT NOT NULL
            );
            "#,
        )?;

        // Create latency_histograms table with all buckets
        let bucket_columns: String = BUCKET_NAMES
            .iter()
            .map(|name| format!("{} BIGINT NOT NULL DEFAULT 0", name))
            .collect::<Vec<_>>()
            .join(",\n                ");

        let histogram_sql = format!(
            r#"
            CREATE TABLE IF NOT EXISTS latency_histograms (
                scenario_id     UUID,

                rel_ns          BIGINT NOT NULL,
                abs_ts          TIMESTAMP NOT NULL,

                {},

                sum_ns          BIGINT NOT NULL DEFAULT 0,
                count           BIGINT NOT NULL DEFAULT 0,

                min_ns          BIGINT,
                max_ns          BIGINT
            );
            "#,
            bucket_columns
        );
        conn.execute_batch(&histogram_sql)?;

        // Create exact_percentiles table for precise percentile storage
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS exact_percentiles (
                scenario_id     UUID,

                rel_ns          BIGINT NOT NULL,
                abs_ts          TIMESTAMP NOT NULL,

                min_ns          BIGINT NOT NULL,
                max_ns          BIGINT NOT NULL,
                mean_ns         DOUBLE NOT NULL,
                stdev_ns        DOUBLE NOT NULL,

                p50_ns          BIGINT NOT NULL,
                p75_ns          BIGINT NOT NULL,
                p90_ns          BIGINT NOT NULL,
                p95_ns          BIGINT NOT NULL,
                p99_ns          BIGINT NOT NULL,
                p999_ns         BIGINT NOT NULL,
                p9999_ns        BIGINT NOT NULL,

                count           BIGINT NOT NULL
            );
            "#,
        )?;

        // Create throughput_counters table
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS throughput_counters (
                scenario_id     UUID,

                rel_ns          BIGINT NOT NULL,
                abs_ts          TIMESTAMP NOT NULL,

                messages_sent   BIGINT NOT NULL,
                messages_recv   BIGINT NOT NULL,
                messages_acked  BIGINT NOT NULL,
                messages_timed_out BIGINT NOT NULL,
                bytes_sent      BIGINT NOT NULL,
                bytes_recv      BIGINT NOT NULL,
                errors          BIGINT NOT NULL
            );
            "#,
        )?;

        // Create container_samples table
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS container_samples (
                run_id          UUID,

                rel_ns          BIGINT NOT NULL,
                abs_ts          TIMESTAMP NOT NULL,
                phase           VARCHAR NOT NULL,

                cpu_percent         DOUBLE NOT NULL,
                cpu_total_ns        BIGINT NOT NULL,
                cpu_system_ns       BIGINT NOT NULL,
                cpu_online          INTEGER NOT NULL,

                memory_usage_bytes  BIGINT NOT NULL,
                memory_limit_bytes  BIGINT NOT NULL,

                network_rx_bytes    BIGINT NOT NULL,
                network_tx_bytes    BIGINT NOT NULL
            );
            "#,
        )?;

        // Create final_results table for aggregated scenario results
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS final_results (
                scenario_id     UUID PRIMARY KEY,

                total_sent      BIGINT NOT NULL,
                total_received  BIGINT NOT NULL,
                total_acked     BIGINT NOT NULL,
                total_timed_out BIGINT NOT NULL,
                total_errors    BIGINT NOT NULL,

                avg_send_rate   DOUBLE NOT NULL,
                avg_recv_rate   DOUBLE NOT NULL,
                delivery_rate   DOUBLE NOT NULL,

                min_ns          BIGINT NOT NULL,
                max_ns          BIGINT NOT NULL,
                mean_ns         DOUBLE NOT NULL,
                stdev_ns        DOUBLE NOT NULL,

                p50_ns          BIGINT NOT NULL,
                p75_ns          BIGINT NOT NULL,
                p90_ns          BIGINT NOT NULL,
                p95_ns          BIGINT NOT NULL,
                p99_ns          BIGINT NOT NULL,
                p999_ns         BIGINT NOT NULL,
                p9999_ns        BIGINT NOT NULL
            );
            "#,
        )?;

        debug!("Database schema initialized");
        Ok(())
    }

    /// Insert a new run record
    pub fn insert_run(&self, run: &RunRecord) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            r#"
            INSERT INTO runs (
                run_id, started_at, broker_host, broker_port,
                broker_name, broker_version, container_id, container_name,
                hostname, cpus, memory_bytes, git_commit, notes
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
            params![
                run.run_id.to_string(),
                run.started_at.to_rfc3339(),
                run.broker_host,
                run.broker_port,
                run.broker_name,
                run.broker_version,
                run.container_id,
                run.container_name,
                run.hostname,
                run.cpus,
                run.memory_bytes,
                run.git_commit,
                run.notes,
            ],
        )?;
        Ok(())
    }

    /// Update run end time
    pub fn update_run_ended(&self, run_id: Uuid, ended_at: DateTime<Utc>) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE runs SET ended_at = ? WHERE run_id = ?",
            params![ended_at.to_rfc3339(), run_id.to_string()],
        )?;
        Ok(())
    }

    /// Insert a new scenario record
    pub fn insert_scenario(&self, scenario: &ScenarioRecord) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            r#"
            INSERT INTO scenarios (
                scenario_id, run_id, started_at, scenario_name, qos,
                publishers, subscribers, topics, msg_rate, payload_size,
                warmup_ns, duration_ns
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
            params![
                scenario.scenario_id.to_string(),
                scenario.run_id.to_string(),
                scenario.started_at.to_rfc3339(),
                scenario.scenario_name,
                scenario.qos,
                scenario.publishers,
                scenario.subscribers,
                scenario.topics,
                scenario.msg_rate,
                scenario.payload_size,
                scenario.warmup_ns,
                scenario.duration_ns,
            ],
        )?;
        Ok(())
    }

    /// Update scenario end time
    pub fn update_scenario_ended(&self, scenario_id: Uuid, ended_at: DateTime<Utc>) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE scenarios SET ended_at = ? WHERE scenario_id = ?",
            params![ended_at.to_rfc3339(), scenario_id.to_string()],
        )?;
        Ok(())
    }

    /// Insert latency histogram checkpoint
    pub fn insert_histogram(
        &self,
        scenario_id: Uuid,
        rel_ns: u64,
        abs_ts: DateTime<Utc>,
        snapshot: &HistogramSnapshot,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();

        // Build dynamic SQL for bucket columns
        let bucket_placeholders: String = (0..NUM_BUCKETS).map(|_| "?").collect::<Vec<_>>().join(", ");
        let bucket_columns: String = BUCKET_NAMES.join(", ");

        let sql = format!(
            r#"
            INSERT INTO latency_histograms (
                scenario_id, rel_ns, abs_ts, {}, sum_ns, count, min_ns, max_ns
            ) VALUES (?, ?, ?, {}, ?, ?, ?, ?)
            "#,
            bucket_columns, bucket_placeholders
        );

        let mut stmt = conn.prepare(&sql)?;

        // Build params dynamically
        let scenario_str = scenario_id.to_string();
        let abs_ts_str = abs_ts.to_rfc3339();

        // Using raw execute with array of values
        let mut values: Vec<duckdb::types::Value> = vec![
            duckdb::types::Value::Text(scenario_str),
            duckdb::types::Value::BigInt(rel_ns as i64),
            duckdb::types::Value::Text(abs_ts_str),
        ];

        for &bucket in &snapshot.buckets {
            values.push(duckdb::types::Value::BigInt(bucket as i64));
        }

        values.push(duckdb::types::Value::BigInt(snapshot.sum_ns as i64));
        values.push(duckdb::types::Value::BigInt(snapshot.count as i64));
        values.push(snapshot.min_ns.map_or(duckdb::types::Value::Null, |v| {
            duckdb::types::Value::BigInt(v as i64)
        }));
        values.push(snapshot.max_ns.map_or(duckdb::types::Value::Null, |v| {
            duckdb::types::Value::BigInt(v as i64)
        }));

        stmt.execute(duckdb::params_from_iter(values))?;
        Ok(())
    }

    /// Insert exact percentiles checkpoint
    pub fn insert_exact_percentiles(
        &self,
        scenario_id: Uuid,
        rel_ns: u64,
        abs_ts: DateTime<Utc>,
        percentiles: &ExactPercentiles,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            r#"
            INSERT INTO exact_percentiles (
                scenario_id, rel_ns, abs_ts,
                min_ns, max_ns, mean_ns, stdev_ns,
                p50_ns, p75_ns, p90_ns, p95_ns, p99_ns, p999_ns, p9999_ns,
                count
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
            params![
                scenario_id.to_string(),
                rel_ns as i64,
                abs_ts.to_rfc3339(),
                percentiles.min_ns as i64,
                percentiles.max_ns as i64,
                percentiles.mean_ns,
                percentiles.stdev_ns,
                percentiles.p50_ns as i64,
                percentiles.p75_ns as i64,
                percentiles.p90_ns as i64,
                percentiles.p95_ns as i64,
                percentiles.p99_ns as i64,
                percentiles.p999_ns as i64,
                percentiles.p9999_ns as i64,
                percentiles.count as i64,
            ],
        )?;
        Ok(())
    }

    /// Insert throughput counters checkpoint
    pub fn insert_counters(
        &self,
        scenario_id: Uuid,
        rel_ns: u64,
        abs_ts: DateTime<Utc>,
        counters: &CounterSnapshot,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            r#"
            INSERT INTO throughput_counters (
                scenario_id, rel_ns, abs_ts,
                messages_sent, messages_recv, messages_acked, messages_timed_out,
                bytes_sent, bytes_recv, errors
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
            params![
                scenario_id.to_string(),
                rel_ns as i64,
                abs_ts.to_rfc3339(),
                counters.messages_sent as i64,
                counters.messages_received as i64,
                counters.messages_acked as i64,
                counters.messages_timed_out as i64,
                counters.bytes_sent as i64,
                counters.bytes_received as i64,
                counters.errors as i64,
            ],
        )?;
        Ok(())
    }

    /// Insert container stats sample
    pub fn insert_container_sample(&self, run_id: Uuid, sample: &ContainerSample) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            r#"
            INSERT INTO container_samples (
                run_id, rel_ns, abs_ts, phase,
                cpu_percent, cpu_total_ns, cpu_system_ns, cpu_online,
                memory_usage_bytes, memory_limit_bytes,
                network_rx_bytes, network_tx_bytes
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
            params![
                run_id.to_string(),
                sample.rel_ns as i64,
                sample.abs_ts.to_rfc3339(),
                sample.phase,
                sample.cpu_percent,
                sample.cpu_total_ns as i64,
                sample.cpu_system_ns as i64,
                sample.cpu_online,
                sample.memory_usage_bytes as i64,
                sample.memory_limit_bytes as i64,
                sample.network_rx_bytes as i64,
                sample.network_tx_bytes as i64,
            ],
        )?;
        Ok(())
    }

    /// Insert final aggregated results
    pub fn insert_final_results(&self, results: &FinalResults) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            r#"
            INSERT INTO final_results (
                scenario_id,
                total_sent, total_received, total_acked, total_timed_out, total_errors,
                avg_send_rate, avg_recv_rate, delivery_rate,
                min_ns, max_ns, mean_ns, stdev_ns,
                p50_ns, p75_ns, p90_ns, p95_ns, p99_ns, p999_ns, p9999_ns
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
            params![
                results.scenario_id.to_string(),
                results.total_sent as i64,
                results.total_received as i64,
                results.total_acked as i64,
                results.total_timed_out as i64,
                results.total_errors as i64,
                results.avg_send_rate,
                results.avg_recv_rate,
                results.delivery_rate,
                results.min_ns as i64,
                results.max_ns as i64,
                results.mean_ns,
                results.stdev_ns,
                results.p50_ns as i64,
                results.p75_ns as i64,
                results.p90_ns as i64,
                results.p95_ns as i64,
                results.p99_ns as i64,
                results.p999_ns as i64,
                results.p9999_ns as i64,
            ],
        )?;
        Ok(())
    }

    /// List recent runs
    pub fn list_runs(&self, limit: u32) -> Result<Vec<RunSummary>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT run_id, CAST(started_at AS VARCHAR), broker_name, broker_host, notes
            FROM runs
            ORDER BY started_at DESC
            LIMIT ?
            "#,
        )?;

        let rows = stmt.query_map([limit], |row| {
            Ok(RunSummary {
                run_id: row.get::<_, String>(0)?.parse().unwrap_or_default(),
                started_at: row.get(1)?,
                broker_name: row.get(2)?,
                broker_host: row.get(3)?,
                notes: row.get(4)?,
            })
        })?;

        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    /// Get run details with scenarios
    pub fn get_run_with_scenarios(&self, run_id: Uuid) -> Result<Option<RunWithScenarios>> {
        let conn = self.conn.lock().unwrap();

        // Get run
        let mut stmt = conn.prepare(
            r#"
            SELECT run_id, CAST(started_at AS VARCHAR), CAST(ended_at AS VARCHAR), broker_host, broker_port,
                   broker_name, broker_version, container_name, hostname, notes
            FROM runs WHERE run_id = ?
            "#,
        )?;

        let run_id_str = run_id.to_string();
        let run = stmt
            .query_row([&run_id_str], |row| {
                Ok(RunDetails {
                    run_id: row.get::<_, String>(0)?.parse().unwrap_or_default(),
                    started_at: row.get(1)?,
                    ended_at: row.get(2)?,
                    broker_host: row.get(3)?,
                    broker_port: row.get(4)?,
                    broker_name: row.get(5)?,
                    broker_version: row.get(6)?,
                    container_name: row.get(7)?,
                    hostname: row.get(8)?,
                    notes: row.get(9)?,
                })
            })
            .ok();

        let run = match run {
            Some(r) => r,
            None => return Ok(None),
        };

        // Get scenarios with final results
        let mut stmt = conn.prepare(
            r#"
            SELECT s.scenario_id, s.scenario_name, s.qos, s.publishers, s.subscribers,
                   s.topics, s.msg_rate, s.payload_size,
                   f.total_sent, f.total_received, f.avg_recv_rate,
                   f.p50_ns, f.p95_ns, f.p99_ns, f.p999_ns, f.delivery_rate
            FROM scenarios s
            LEFT JOIN final_results f ON s.scenario_id = f.scenario_id
            WHERE s.run_id = ?
            "#,
        )?;

        let scenarios = stmt
            .query_map([&run_id_str], |row| {
                Ok(ScenarioSummary {
                    scenario_id: row.get::<_, String>(0)?.parse().unwrap_or_default(),
                    scenario_name: row.get(1)?,
                    qos: row.get(2)?,
                    publishers: row.get(3)?,
                    subscribers: row.get(4)?,
                    topics: row.get(5)?,
                    msg_rate: row.get(6)?,
                    payload_size: row.get(7)?,
                    total_sent: row.get(8).unwrap_or(0),
                    total_received: row.get(9).unwrap_or(0),
                    avg_recv_rate: row.get(10).unwrap_or(0.0),
                    p50_ns: row.get(11).unwrap_or(0),
                    p95_ns: row.get(12).unwrap_or(0),
                    p99_ns: row.get(13).unwrap_or(0),
                    p999_ns: row.get(14).unwrap_or(0),
                    delivery_rate: row.get(15).unwrap_or(0.0),
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(Some(RunWithScenarios { run, scenarios }))
    }

    /// Export tables to Parquet
    pub fn export_to_parquet(&self, output_dir: &Path) -> Result<()> {
        let conn = self.conn.lock().unwrap();

        let tables = [
            "runs",
            "scenarios",
            "latency_histograms",
            "exact_percentiles",
            "throughput_counters",
            "container_samples",
            "final_results",
        ];

        for table in tables {
            let output_path = output_dir.join(format!("{}.parquet", table));
            let sql = format!(
                "COPY {} TO '{}' (FORMAT PARQUET)",
                table,
                output_path.display()
            );
            conn.execute(&sql, [])?;
            info!("Exported {} to {}", table, output_path.display());
        }

        Ok(())
    }
}

/// Run record for insertion
#[derive(Debug, Clone)]
pub struct RunRecord {
    pub run_id: Uuid,
    pub started_at: DateTime<Utc>,
    pub broker_host: String,
    pub broker_port: u16,
    pub broker_name: Option<String>,
    pub broker_version: Option<String>,
    pub container_id: Option<String>,
    pub container_name: Option<String>,
    pub hostname: Option<String>,
    pub cpus: Option<i32>,
    pub memory_bytes: Option<i64>,
    pub git_commit: Option<String>,
    pub notes: Option<String>,
}

/// Scenario record for insertion
#[derive(Debug, Clone)]
pub struct ScenarioRecord {
    pub scenario_id: Uuid,
    pub run_id: Uuid,
    pub started_at: DateTime<Utc>,
    pub scenario_name: String,
    pub qos: i32,
    pub publishers: i32,
    pub subscribers: i32,
    pub topics: i32,
    pub msg_rate: i32,
    pub payload_size: i32,
    pub warmup_ns: i64,
    pub duration_ns: i64,
}

/// Final aggregated results
#[derive(Debug, Clone)]
pub struct FinalResults {
    pub scenario_id: Uuid,
    pub total_sent: u64,
    pub total_received: u64,
    pub total_acked: u64,
    pub total_timed_out: u64,
    pub total_errors: u64,
    pub avg_send_rate: f64,
    pub avg_recv_rate: f64,
    pub delivery_rate: f64,
    pub min_ns: u64,
    pub max_ns: u64,
    pub mean_ns: f64,
    pub stdev_ns: f64,
    pub p50_ns: u64,
    pub p75_ns: u64,
    pub p90_ns: u64,
    pub p95_ns: u64,
    pub p99_ns: u64,
    pub p999_ns: u64,
    pub p9999_ns: u64,
}

/// Run summary for listing
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct RunSummary {
    pub run_id: Uuid,
    pub started_at: String,
    pub broker_name: Option<String>,
    pub broker_host: String,
    pub notes: Option<String>,
}

/// Run details
#[derive(Debug, Clone)]
pub struct RunDetails {
    pub run_id: Uuid,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub broker_host: String,
    pub broker_port: i32,
    pub broker_name: Option<String>,
    pub broker_version: Option<String>,
    pub container_name: Option<String>,
    pub hostname: Option<String>,
    pub notes: Option<String>,
}

/// Scenario summary
#[derive(Debug, Clone)]
pub struct ScenarioSummary {
    pub scenario_id: Uuid,
    pub scenario_name: String,
    pub qos: i32,
    pub publishers: i32,
    pub subscribers: i32,
    pub topics: i32,
    pub msg_rate: i32,
    pub payload_size: i32,
    pub total_sent: i64,
    pub total_received: i64,
    pub avg_recv_rate: f64,
    pub p50_ns: i64,
    pub p95_ns: i64,
    pub p99_ns: i64,
    pub p999_ns: i64,
    pub delivery_rate: f64,
}

/// Run with scenarios
#[derive(Debug, Clone)]
pub struct RunWithScenarios {
    pub run: RunDetails,
    pub scenarios: Vec<ScenarioSummary>,
}
