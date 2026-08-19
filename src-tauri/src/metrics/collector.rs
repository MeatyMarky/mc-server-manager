//! Resource sampling.
//!
//! **One collector, not one task per server.** A single loop refreshes the
//! process table once per tick and writes a row per running instance, so ten
//! servers cost one `System::refresh` rather than ten. The cost is the same
//! whether one instance is running or twenty.

use std::collections::HashMap;
use std::time::Duration;

use serde::Serialize;
use sqlx::SqlitePool;
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
use ts_rs::TS;

use crate::db::now_rfc3339;
use crate::error::AppResult;
use crate::state::AppState;

/// Sampling interval when the setting is unset.
pub const DEFAULT_INTERVAL_SECONDS: u64 = 5;
pub const INTERVAL_SETTING: &str = "metrics_interval_seconds";

/// One sample, as the charts read it.
#[derive(Debug, Clone, PartialEq, Serialize, sqlx::FromRow, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct Sample {
    pub ts: String,
    pub cpu_pct: f64,
    #[ts(type = "number")]
    pub rss_bytes: i64,
    #[ts(type = "number | null")]
    pub players: Option<i64>,
}

/// A chart window, which decides both the range and which resolution tier the
/// rows come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub enum Window {
    Hour,
    Day,
    Week,
    Month,
}

impl Window {
    pub fn duration(self) -> chrono::Duration {
        match self {
            Window::Hour => chrono::Duration::hours(1),
            Window::Day => chrono::Duration::hours(24),
            Window::Week => chrono::Duration::days(7),
            Window::Month => chrono::Duration::days(30),
        }
    }

    /// Seconds between the points a chart wants. Beyond 24 h the stored rows are
    /// already one per minute (see the retention policy), so asking for finer
    /// buckets than this would only invent detail that no longer exists.
    pub fn bucket_seconds(self) -> i64 {
        match self {
            Window::Hour => 0,     // full resolution, as stored
            Window::Day => 60,     // a point a minute
            Window::Week => 600,   // ten minutes
            Window::Month => 3600, // an hour
        }
    }
}

/// What the sampler read for one process.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Reading {
    pub cpu_pct: f64,
    pub rss_bytes: u64,
}

/// Turns a raw CPU reading into a percentage of one core.
///
/// `sysinfo` reports CPU usage summed across cores, which on a 16-core machine
/// gives figures like 400%. Servers are mostly single-threaded, so the number
/// people expect is per-core; it is clamped to a sane range rather than shown raw.
pub fn normalise_cpu(raw: f32) -> f64 {
    (raw as f64).clamp(0.0, 6_400.0)
}

/// Reads every pid in one pass.
pub fn sample_processes(system: &mut System, pids: &[u32]) -> HashMap<u32, Reading> {
    if pids.is_empty() {
        return HashMap::new();
    }

    let wanted: Vec<Pid> = pids.iter().map(|pid| Pid::from_u32(*pid)).collect();
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&wanted),
        true,
        ProcessRefreshKind::nothing().with_cpu().with_memory(),
    );

    pids.iter()
        .filter_map(|pid| {
            let process = system.process(Pid::from_u32(*pid))?;
            Some((
                *pid,
                Reading {
                    cpu_pct: normalise_cpu(process.cpu_usage()),
                    rss_bytes: process.memory(),
                },
            ))
        })
        .collect()
}

/// Writes one sample per running instance.
pub async fn record(
    pool: &SqlitePool,
    samples: &[(i64, Reading, Option<i64>)],
) -> AppResult<()> {
    let ts = now_rfc3339();
    for (instance_id, reading, players) in samples {
        sqlx::query(
            "INSERT INTO resource_samples (instance_id, ts, cpu_pct, rss_bytes, players)
             VALUES (?, ?, ?, ?, ?)
             ON CONFLICT(instance_id, ts) DO UPDATE SET
                cpu_pct = excluded.cpu_pct, rss_bytes = excluded.rss_bytes,
                players = excluded.players",
        )
        .bind(instance_id)
        .bind(&ts)
        .bind(reading.cpu_pct)
        .bind(reading.rss_bytes as i64)
        .bind(players)
        .execute(pool)
        .await?;
    }
    Ok(())
}

/// The configured interval, or the default.
pub async fn interval(pool: &SqlitePool) -> Duration {
    let seconds = crate::db::setting_get(pool, INTERVAL_SETTING)
        .await
        .ok()
        .flatten()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(DEFAULT_INTERVAL_SECONDS)
        .clamp(1, 300);
    Duration::from_secs(seconds)
}

/// The one sampling loop. Started once at launch, whatever the instance count.
pub async fn run(app: tauri::AppHandle) {
    use tauri::Manager;

    let mut system = System::new();

    loop {
        let state = app.state::<AppState>();
        let delay = interval(&state.db).await;

        let live = state.live_uuids();
        if !live.is_empty() {
            if let Err(err) = tick(&app, &mut system, &live).await {
                tracing::warn!(error = %err, "a metrics tick failed");
            }
        }

        tokio::time::sleep(delay).await;
    }
}

async fn tick(app: &tauri::AppHandle, system: &mut System, live: &[String]) -> AppResult<()> {
    use tauri::Manager;
    let state = app.state::<AppState>();

    // pid per live instance, skipping any whose pid we do not know.
    let mut targets: Vec<(i64, String, u32)> = Vec::new();
    for uuid in live {
        let Ok(row) = crate::instance::get_by_uuid(&state.db, uuid).await else {
            continue;
        };
        let pid = state
            .supervisor
            .pid_of(uuid)
            .or_else(|| row.pid.and_then(|pid| u32::try_from(pid).ok()));
        if let Some(pid) = pid {
            targets.push((row.id, uuid.clone(), pid));
        }
    }
    if targets.is_empty() {
        return Ok(());
    }

    // One refresh for every instance, which is the point of a single collector.
    let pids: Vec<u32> = targets.iter().map(|(_, _, pid)| *pid).collect();
    let readings = sample_processes(system, &pids);

    let mut rows = Vec::new();
    for (id, uuid, pid) in &targets {
        let Some(reading) = readings.get(pid) else {
            continue;
        };
        let players = state.supervisor.online_count(uuid).map(|count| count as i64);
        rows.push((*id, *reading, players));

        crate::events::metrics(
            app,
            uuid,
            crate::events::MetricsEvent {
                uuid: uuid.clone(),
                ts: now_rfc3339(),
                cpu_pct: reading.cpu_pct,
                rss_bytes: reading.rss_bytes as i64,
                players,
            },
        );
    }

    record(&state.db, &rows).await
}

/// Samples for one instance over a window, at the resolution that window wants.
pub async fn range(
    pool: &SqlitePool,
    instance_id: i64,
    window: Window,
    now: chrono::DateTime<chrono::Utc>,
) -> AppResult<Vec<Sample>> {
    let since = (now - window.duration()).to_rfc3339_opts(chrono::SecondsFormat::Secs, true);

    let rows = sqlx::query_as::<_, Sample>(
        "SELECT ts, cpu_pct, rss_bytes, players FROM resource_samples
         WHERE instance_id = ? AND ts >= ? ORDER BY ts",
    )
    .bind(instance_id)
    .bind(&since)
    .fetch_all(pool)
    .await?;

    Ok(downsample(rows, window.bucket_seconds()))
}

/// Averages samples into buckets, so a month of data is a few hundred points
/// rather than tens of thousands the chart would have to throw away anyway.
pub fn downsample(samples: Vec<Sample>, bucket_seconds: i64) -> Vec<Sample> {
    if bucket_seconds <= 0 || samples.is_empty() {
        return samples;
    }

    let mut out: Vec<Sample> = Vec::new();
    let mut bucket: Vec<Sample> = Vec::new();
    let mut bucket_key: Option<i64> = None;

    let key_of = |sample: &Sample| -> Option<i64> {
        chrono::DateTime::parse_from_rfc3339(&sample.ts)
            .ok()
            .map(|ts| ts.timestamp() / bucket_seconds)
    };

    for sample in samples {
        let key = key_of(&sample);
        if bucket_key.is_some() && key != bucket_key {
            out.push(fold(&bucket));
            bucket.clear();
        }
        bucket_key = key;
        bucket.push(sample);
    }
    if !bucket.is_empty() {
        out.push(fold(&bucket));
    }
    out
}

/// One bucket becomes one point: mean CPU, peak memory (the number that matters
/// for a heap), and peak player count.
fn fold(bucket: &[Sample]) -> Sample {
    let count = bucket.len().max(1) as f64;
    Sample {
        ts: bucket[0].ts.clone(),
        cpu_pct: bucket.iter().map(|sample| sample.cpu_pct).sum::<f64>() / count,
        rss_bytes: bucket.iter().map(|sample| sample.rss_bytes).max().unwrap_or(0),
        players: bucket.iter().filter_map(|sample| sample.players).max(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(ts: &str, cpu: f64, rss: i64, players: Option<i64>) -> Sample {
        Sample {
            ts: ts.to_string(),
            cpu_pct: cpu,
            rss_bytes: rss,
            players,
        }
    }

    #[test]
    fn windows_pick_a_resolution_that_matches_what_is_stored() {
        // Inside 24 h the rows are full resolution; past it they are already
        // per-minute, so the buckets never ask for finer detail than exists.
        assert_eq!(Window::Hour.bucket_seconds(), 0);
        assert_eq!(Window::Day.bucket_seconds(), 60);
        assert_eq!(Window::Week.bucket_seconds(), 600);
        assert_eq!(Window::Month.bucket_seconds(), 3600);

        assert_eq!(Window::Day.duration(), chrono::Duration::hours(24));
        assert_eq!(Window::Month.duration(), chrono::Duration::days(30));
    }

    #[test]
    fn cpu_readings_are_clamped_but_not_squashed() {
        assert_eq!(normalise_cpu(0.0), 0.0);
        assert_eq!(normalise_cpu(87.5), 87.5);
        // Multi-core totals above 100% are real and kept.
        assert_eq!(normalise_cpu(240.0), 240.0);
        assert_eq!(normalise_cpu(-1.0), 0.0);
    }

    #[test]
    fn downsampling_averages_cpu_and_keeps_the_peaks() {
        let samples = vec![
            sample("2026-08-18T12:00:00Z", 10.0, 1_000, Some(1)),
            sample("2026-08-18T12:00:30Z", 30.0, 3_000, Some(4)),
            sample("2026-08-18T12:01:00Z", 50.0, 2_000, Some(2)),
        ];

        let folded = downsample(samples, 60);
        assert_eq!(folded.len(), 2, "two minute buckets");
        assert_eq!(folded[0].cpu_pct, 20.0, "mean CPU");
        assert_eq!(folded[0].rss_bytes, 3_000, "peak memory");
        assert_eq!(folded[0].players, Some(4), "peak players");
        assert_eq!(folded[1].cpu_pct, 50.0);
    }

    #[test]
    fn a_zero_bucket_returns_the_samples_untouched() {
        let samples = vec![sample("2026-08-18T12:00:00Z", 10.0, 1_000, None)];
        assert_eq!(downsample(samples.clone(), 0), samples);
        assert!(downsample(Vec::new(), 60).is_empty());
    }

    #[test]
    fn this_process_can_be_sampled() {
        let mut system = System::new();
        let me = std::process::id();
        let readings = sample_processes(&mut system, &[me]);

        let reading = readings.get(&me).expect("the test process is visible");
        assert!(reading.rss_bytes > 0, "memory is reported");
        assert!(reading.cpu_pct >= 0.0);
    }

    #[test]
    fn a_dead_pid_produces_no_reading_rather_than_a_zero_row() {
        let mut system = System::new();
        assert!(sample_processes(&mut system, &[4_294_967_280]).is_empty());
        assert!(sample_processes(&mut system, &[]).is_empty());
    }

    #[tokio::test]
    async fn samples_are_written_and_read_back_within_the_window() {
        let pool = crate::db::connect_in_memory().await.unwrap();
        let now = crate::db::now_rfc3339();
        sqlx::query(
            "INSERT INTO instances (uuid, name, path, server_type, mc_version, launch_kind,
                jvm_args, server_args, created_at, updated_at)
             VALUES ('u1', 'A', 'Z:/a', 'paper', '1.21.4', 'jar', '[]', '[]', ?, ?)",
        )
        .bind(&now)
        .bind(&now)
        .execute(&pool)
        .await
        .unwrap();

        record(
            &pool,
            &[(
                1,
                Reading {
                    cpu_pct: 42.0,
                    rss_bytes: 2_048,
                },
                Some(3),
            )],
        )
        .await
        .unwrap();

        let samples = range(&pool, 1, Window::Hour, chrono::Utc::now()).await.unwrap();
        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0].cpu_pct, 42.0);
        assert_eq!(samples[0].rss_bytes, 2_048);
        assert_eq!(samples[0].players, Some(3));

        // A sample outside the window is not returned.
        sqlx::query(
            "INSERT INTO resource_samples (instance_id, ts, cpu_pct, rss_bytes, players)
             VALUES (1, ?, 1.0, 1, 0)",
        )
        .bind(
            (chrono::Utc::now() - chrono::Duration::days(2))
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        )
        .execute(&pool)
        .await
        .unwrap();

        assert_eq!(
            range(&pool, 1, Window::Hour, chrono::Utc::now()).await.unwrap().len(),
            1
        );
        assert_eq!(
            range(&pool, 1, Window::Week, chrono::Utc::now()).await.unwrap().len(),
            2
        );
    }

    #[tokio::test]
    async fn the_interval_is_configurable_and_clamped() {
        let pool = crate::db::connect_in_memory().await.unwrap();
        assert_eq!(
            interval(&pool).await,
            Duration::from_secs(DEFAULT_INTERVAL_SECONDS)
        );

        crate::db::setting_set(&pool, INTERVAL_SETTING, "30").await.unwrap();
        assert_eq!(interval(&pool).await, Duration::from_secs(30));

        crate::db::setting_set(&pool, INTERVAL_SETTING, "0").await.unwrap();
        assert_eq!(interval(&pool).await, Duration::from_secs(1), "never zero");

        crate::db::setting_set(&pool, INTERVAL_SETTING, "nonsense").await.unwrap();
        assert_eq!(
            interval(&pool).await,
            Duration::from_secs(DEFAULT_INTERVAL_SECONDS)
        );
    }
}
