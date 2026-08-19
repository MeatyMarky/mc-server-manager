//! Scheduled backups.
//!
//! Two shapes of schedule: every N minutes, or a daily time (`03:30`) — the
//! "cron-ish" case people actually ask for. Two rules that matter more than the
//! syntax:
//!
//!   * **Missed runs never queue up.** An app closed for a week owes one backup,
//!     not two hundred. Overdue means "run once, now".
//!   * **Retention is enforced after every run**, by count and by age together:
//!     an archive has to fall outside both limits before it is deleted.

use chrono::{DateTime, Duration, NaiveTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::db::now_rfc3339;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

use super::{archive::Format, archive::Scope, Backup};

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct Schedule {
    #[ts(type = "number")]
    pub id: i64,
    #[ts(type = "number")]
    pub instance_id: i64,
    /// Daily time as `HH:MM`, when this is a daily schedule.
    pub cron: Option<String>,
    /// Minutes between runs, when this is an interval schedule.
    #[ts(type = "number | null")]
    pub interval_minutes: Option<i64>,
    pub scope: Scope,
    pub format: Format,
    #[ts(type = "number | null")]
    pub compression_level: Option<i64>,
    #[ts(type = "number | null")]
    pub keep_count: Option<i64>,
    #[ts(type = "number | null")]
    pub keep_days: Option<i64>,
    pub enabled: bool,
    /// Restart the server once the backup is done.
    pub restart_after: bool,
    /// Skip the run when nobody has been online since the last one.
    pub skip_if_idle: bool,
    pub last_run_at: Option<String>,
    pub next_run_at: Option<String>,
}

/// The fields the UI sends when creating or editing a schedule.
#[derive(Debug, Clone, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct ScheduleInput {
    #[ts(type = "number | null")]
    pub id: Option<i64>,
    pub cron: Option<String>,
    #[ts(type = "number | null")]
    pub interval_minutes: Option<i64>,
    pub scope: Scope,
    pub format: Format,
    #[ts(type = "number | null")]
    pub compression_level: Option<i64>,
    #[ts(type = "number | null")]
    pub keep_count: Option<i64>,
    #[ts(type = "number | null")]
    pub keep_days: Option<i64>,
    pub enabled: bool,
    pub restart_after: bool,
    pub skip_if_idle: bool,
}

/// Parses `HH:MM`.
pub fn parse_daily_time(value: &str) -> Option<NaiveTime> {
    let (hours, minutes) = value.trim().split_once(':')?;
    NaiveTime::from_hms_opt(hours.trim().parse().ok()?, minutes.trim().parse().ok()?, 0)
}

/// When a schedule should next run, given when it last did.
///
/// A schedule that is already overdue returns `now`: the caller runs it once and
/// moves on, which is what stops a closed app from owing a backlog.
pub fn next_run(
    schedule: &Schedule,
    last_run: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    if !schedule.enabled {
        return None;
    }

    if let Some(minutes) = schedule.interval_minutes.filter(|minutes| *minutes > 0) {
        let due = match last_run {
            Some(last) => last + Duration::minutes(minutes),
            // Never run: due now, so a new schedule takes its first backup.
            None => now,
        };
        return Some(due);
    }

    if let Some(time) = schedule.cron.as_deref().and_then(parse_daily_time) {
        let today = now
            .date_naive()
            .and_time(time)
            .and_utc();
        let due = if today > now { today } else { today + Duration::days(1) };

        // Already missed today's run and it has not happened yet: due now.
        if let Some(last) = last_run {
            let todays = now.date_naive().and_time(time).and_utc();
            if todays <= now && last < todays {
                return Some(now);
            }
        } else if today > now {
            return Some(today);
        } else {
            return Some(now);
        }
        return Some(due);
    }

    None
}

/// Whether a schedule is due right now.
pub fn is_due(schedule: &Schedule, now: DateTime<Utc>) -> bool {
    if !schedule.enabled {
        return false;
    }
    let last = schedule
        .last_run_at
        .as_deref()
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.with_timezone(&Utc));

    match next_run(schedule, last, now) {
        Some(due) => due <= now,
        None => false,
    }
}

/// Backups to delete, given the retention limits.
///
/// An archive has to fall outside *both* limits: keeping "the last 5" and "the
/// last 7 days" means an archive survives if either rule wants it. Manual and
/// pre-restore backups are never pruned automatically.
pub fn select_for_pruning(
    backups: &[Backup],
    keep_count: Option<i64>,
    keep_days: Option<i64>,
    now: DateTime<Utc>,
) -> Vec<i64> {
    if keep_count.is_none() && keep_days.is_none() {
        return Vec::new();
    }

    let mut ordered: Vec<&Backup> = backups.iter().collect();
    ordered.sort_by(|a, b| b.created_at.cmp(&a.created_at));

    let mut doomed = Vec::new();
    for (position, backup) in ordered.iter().enumerate() {
        let within_count = keep_count
            .map(|keep| (position as i64) < keep.max(0))
            .unwrap_or(false);

        let within_age = keep_days
            .map(|days| {
                DateTime::parse_from_rfc3339(&backup.created_at)
                    .map(|created| {
                        now.signed_duration_since(created.with_timezone(&Utc))
                            <= Duration::days(days.max(0))
                    })
                    .unwrap_or(true)
            })
            .unwrap_or(false);

        if !within_count && !within_age {
            doomed.push(backup.id);
        }
    }
    doomed
}

pub async fn list(state: &AppState, instance_id: i64) -> AppResult<Vec<Schedule>> {
    let rows = sqlx::query_as::<_, Schedule>(
        "SELECT id, instance_id, cron, interval_minutes, scope, format, compression_level,
                keep_count, keep_days, enabled, restart_after, skip_if_idle,
                last_run_at, next_run_at
         FROM backup_schedules WHERE instance_id = ? ORDER BY id",
    )
    .bind(instance_id)
    .fetch_all(&state.db)
    .await?;
    Ok(rows)
}

pub async fn all_enabled(state: &AppState) -> AppResult<Vec<Schedule>> {
    let rows = sqlx::query_as::<_, Schedule>(
        "SELECT id, instance_id, cron, interval_minutes, scope, format, compression_level,
                keep_count, keep_days, enabled, restart_after, skip_if_idle,
                last_run_at, next_run_at
         FROM backup_schedules WHERE enabled = 1",
    )
    .fetch_all(&state.db)
    .await?;
    Ok(rows)
}

/// Creates or updates a schedule.
pub async fn upsert(state: &AppState, instance_id: i64, input: ScheduleInput) -> AppResult<Schedule> {
    if input.interval_minutes.is_none() && input.cron.is_none() {
        return Err(AppError::Other(
            "a schedule needs either an interval or a daily time".into(),
        ));
    }
    if let Some(cron) = input.cron.as_deref() {
        if parse_daily_time(cron).is_none() {
            return Err(AppError::Other(format!(
                "\"{cron}\" is not a time of day; use HH:MM"
            )));
        }
    }
    if input.interval_minutes.is_some_and(|minutes| minutes < 5) {
        return Err(AppError::Other(
            "backups cannot run more often than every 5 minutes".into(),
        ));
    }

    let id: i64 = match input.id {
        Some(id) => {
            sqlx::query(
                "UPDATE backup_schedules SET cron = ?, interval_minutes = ?, scope = ?, format = ?,
                    compression_level = ?, keep_count = ?, keep_days = ?, enabled = ?,
                    restart_after = ?, skip_if_idle = ? WHERE id = ? AND instance_id = ?",
            )
            .bind(&input.cron)
            .bind(input.interval_minutes)
            .bind(input.scope)
            .bind(input.format)
            .bind(input.compression_level)
            .bind(input.keep_count)
            .bind(input.keep_days)
            .bind(input.enabled)
            .bind(input.restart_after)
            .bind(input.skip_if_idle)
            .bind(id)
            .bind(instance_id)
            .execute(&state.db)
            .await?;
            id
        }
        None => {
            sqlx::query_scalar(
                "INSERT INTO backup_schedules (instance_id, cron, interval_minutes, scope, format,
                    compression_level, keep_count, keep_days, enabled, restart_after, skip_if_idle)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                 RETURNING id",
            )
            .bind(instance_id)
            .bind(&input.cron)
            .bind(input.interval_minutes)
            .bind(input.scope)
            .bind(input.format)
            .bind(input.compression_level)
            .bind(input.keep_count)
            .bind(input.keep_days)
            .bind(input.enabled)
            .bind(input.restart_after)
            .bind(input.skip_if_idle)
            .fetch_one(&state.db)
            .await?
        }
    };

    let schedules = list(state, instance_id).await?;
    schedules
        .into_iter()
        .find(|schedule| schedule.id == id)
        .ok_or_else(|| AppError::Other("the schedule could not be read back".into()))
}

pub async fn delete(state: &AppState, schedule_id: i64) -> AppResult<()> {
    sqlx::query("DELETE FROM backup_schedules WHERE id = ?")
        .bind(schedule_id)
        .execute(&state.db)
        .await?;
    Ok(())
}

/// Records that a schedule ran, and when it is next due.
pub async fn mark_ran(state: &AppState, schedule: &Schedule, now: DateTime<Utc>) -> AppResult<()> {
    let next = next_run(schedule, Some(now), now).map(|due| {
        due.max(now + Duration::minutes(1))
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    });

    sqlx::query("UPDATE backup_schedules SET last_run_at = ?, next_run_at = ? WHERE id = ?")
        .bind(now_rfc3339())
        .bind(next)
        .bind(schedule.id)
        .execute(&state.db)
        .await?;
    Ok(())
}

/// True when nobody has been online since the schedule last ran, so the run can
/// be skipped: an idle server otherwise piles up identical archives.
pub async fn instance_was_idle(state: &AppState, schedule: &Schedule) -> AppResult<bool> {
    let Some(last_run) = schedule.last_run_at.as_deref() else {
        // Never run: take the first backup regardless.
        return Ok(false);
    };

    let seen: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM players_seen WHERE instance_id = ? AND last_seen > ?",
    )
    .bind(schedule.instance_id)
    .bind(last_run)
    .fetch_one(&state.db)
    .await?;

    Ok(seen == 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schedule(interval: Option<i64>, cron: Option<&str>) -> Schedule {
        Schedule {
            id: 1,
            instance_id: 1,
            cron: cron.map(str::to_string),
            interval_minutes: interval,
            scope: Scope::Full,
            format: Format::TarZst,
            compression_level: None,
            keep_count: Some(5),
            keep_days: Some(7),
            enabled: true,
            restart_after: false,
            skip_if_idle: false,
            last_run_at: None,
            next_run_at: None,
        }
    }

    fn at(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text).unwrap().with_timezone(&Utc)
    }

    fn backup(id: i64, created_at: &str) -> Backup {
        Backup {
            id,
            instance_id: 1,
            path: format!("/backups/{id}.tar.zst"),
            format: Format::TarZst,
            scope: Scope::Full,
            kind: "scheduled".into(),
            label: None,
            size_bytes: 1024,
            created_at: created_at.to_string(),
        }
    }

    #[test]
    fn daily_times_parse_or_are_rejected() {
        assert!(parse_daily_time("03:30").is_some());
        assert!(parse_daily_time(" 23:59 ").is_some());
        assert!(parse_daily_time("24:00").is_none());
        assert!(parse_daily_time("3.30").is_none());
        assert!(parse_daily_time("").is_none());
    }

    #[test]
    fn an_interval_schedule_runs_one_interval_after_the_last_run() {
        let mut hourly = schedule(Some(60), None);
        let now = at("2026-08-18T12:00:00Z");

        // Never run: due immediately.
        assert_eq!(next_run(&hourly, None, now), Some(now));

        hourly.last_run_at = Some("2026-08-18T11:30:00Z".into());
        let due = next_run(&hourly, Some(at("2026-08-18T11:30:00Z")), now).unwrap();
        assert_eq!(due, at("2026-08-18T12:30:00Z"));
        assert!(!is_due(&hourly, now));
    }

    #[test]
    fn an_overdue_schedule_owes_exactly_one_run() {
        let mut hourly = schedule(Some(60), None);
        // The app was closed for a week.
        hourly.last_run_at = Some("2026-08-11T12:00:00Z".into());
        let now = at("2026-08-18T12:00:00Z");

        assert!(is_due(&hourly, now), "a week overdue is due");

        // After running once it is due again in an hour, not 168 times over.
        let next = next_run(&hourly, Some(now), now).unwrap();
        assert_eq!(next, at("2026-08-18T13:00:00Z"));
    }

    #[test]
    fn a_daily_schedule_runs_once_at_its_time() {
        let daily = schedule(None, Some("03:30"));

        // Before today's time: due later today.
        let morning = at("2026-08-18T01:00:00Z");
        assert_eq!(next_run(&daily, None, morning), Some(at("2026-08-18T03:30:00Z")));
        assert!(!is_due(&daily, morning));

        // After today's time with no run today: due now, once.
        let evening = at("2026-08-18T20:00:00Z");
        let mut ran_yesterday = daily.clone();
        ran_yesterday.last_run_at = Some("2026-08-17T03:30:00Z".into());
        assert!(is_due(&ran_yesterday, evening));

        // Already ran today: due tomorrow.
        let mut ran_today = daily.clone();
        ran_today.last_run_at = Some("2026-08-18T03:30:05Z".into());
        let next = next_run(&ran_today, Some(at("2026-08-18T03:30:05Z")), evening).unwrap();
        assert_eq!(next.date_naive(), at("2026-08-19T03:30:00Z").date_naive());
        assert!(!is_due(&ran_today, evening));
    }

    #[test]
    fn a_disabled_schedule_is_never_due() {
        let mut disabled = schedule(Some(5), None);
        disabled.enabled = false;
        assert!(!is_due(&disabled, at("2026-08-18T12:00:00Z")));
        assert_eq!(next_run(&disabled, None, at("2026-08-18T12:00:00Z")), None);
    }

    #[test]
    fn retention_keeps_an_archive_that_either_rule_wants() {
        let now = at("2026-08-18T12:00:00Z");
        let backups = vec![
            backup(1, "2026-08-18T11:00:00Z"), // newest
            backup(2, "2026-08-17T11:00:00Z"),
            backup(3, "2026-08-01T11:00:00Z"), // old, but inside "keep 3"
            backup(4, "2026-07-01T11:00:00Z"), // outside both
        ];

        let doomed = select_for_pruning(&backups, Some(3), Some(7), now);
        assert_eq!(doomed, vec![4], "only the archive outside both limits goes");
    }

    #[test]
    fn count_only_retention_keeps_the_newest_n() {
        let now = at("2026-08-18T12:00:00Z");
        let backups = vec![
            backup(1, "2026-08-18T11:00:00Z"),
            backup(2, "2026-08-17T11:00:00Z"),
            backup(3, "2026-08-16T11:00:00Z"),
        ];
        assert_eq!(select_for_pruning(&backups, Some(2), None, now), vec![3]);
    }

    #[test]
    fn age_only_retention_keeps_what_is_recent() {
        let now = at("2026-08-18T12:00:00Z");
        let backups = vec![
            backup(1, "2026-08-18T11:00:00Z"),
            backup(2, "2026-08-01T11:00:00Z"),
        ];
        assert_eq!(select_for_pruning(&backups, None, Some(7), now), vec![2]);
    }

    #[test]
    fn without_limits_nothing_is_pruned() {
        let now = at("2026-08-18T12:00:00Z");
        let backups = vec![backup(1, "2020-01-01T00:00:00Z")];
        assert!(select_for_pruning(&backups, None, None, now).is_empty());
    }

    #[tokio::test]
    async fn schedules_validate_before_they_are_stored() {
        let pool = crate::db::connect_in_memory().await.unwrap();
        let state = AppState::new(pool, std::env::temp_dir());
        let now = now_rfc3339();
        sqlx::query(
            "INSERT INTO instances (uuid, name, path, server_type, mc_version, launch_kind,
                jvm_args, server_args, created_at, updated_at)
             VALUES ('u1', 'A', 'Z:/a', 'paper', '1.21.4', 'jar', '[]', '[]', ?, ?)",
        )
        .bind(&now)
        .bind(&now)
        .execute(&state.db)
        .await
        .unwrap();

        let base = ScheduleInput {
            id: None,
            cron: None,
            interval_minutes: None,
            scope: Scope::Full,
            format: Format::TarZst,
            compression_level: None,
            keep_count: Some(5),
            keep_days: None,
            enabled: true,
            restart_after: false,
            skip_if_idle: false,
        };

        // Neither an interval nor a time.
        assert!(upsert(&state, 1, base.clone()).await.is_err());

        // Too frequent.
        let mut too_often = base.clone();
        too_often.interval_minutes = Some(1);
        assert!(upsert(&state, 1, too_often)
            .await
            .unwrap_err()
            .to_string()
            .contains("5 minutes"));

        // A time that is not a time.
        let mut bad_time = base.clone();
        bad_time.cron = Some("half past three".into());
        assert!(upsert(&state, 1, bad_time)
            .await
            .unwrap_err()
            .to_string()
            .contains("HH:MM"));

        // A valid one round-trips, and editing it keeps the same row.
        let mut good = base.clone();
        good.interval_minutes = Some(60);
        let created = upsert(&state, 1, good).await.unwrap();
        assert_eq!(created.interval_minutes, Some(60));

        let mut edited = base;
        edited.id = Some(created.id);
        edited.interval_minutes = Some(120);
        edited.skip_if_idle = true;
        let updated = upsert(&state, 1, edited).await.unwrap();
        assert_eq!(updated.id, created.id);
        assert_eq!(updated.interval_minutes, Some(120));
        assert!(updated.skip_if_idle);
        assert_eq!(list(&state, 1).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn an_idle_server_is_recognised_from_who_has_been_seen() {
        let pool = crate::db::connect_in_memory().await.unwrap();
        let state = AppState::new(pool, std::env::temp_dir());
        let now = now_rfc3339();
        sqlx::query(
            "INSERT INTO instances (uuid, name, path, server_type, mc_version, launch_kind,
                jvm_args, server_args, created_at, updated_at)
             VALUES ('u1', 'A', 'Z:/a', 'paper', '1.21.4', 'jar', '[]', '[]', ?, ?)",
        )
        .bind(&now)
        .bind(&now)
        .execute(&state.db)
        .await
        .unwrap();

        let mut schedule = schedule(Some(60), None);
        // Never run: the first backup is always taken.
        assert!(!instance_was_idle(&state, &schedule).await.unwrap());

        schedule.last_run_at = Some("2026-08-18T10:00:00Z".into());
        assert!(instance_was_idle(&state, &schedule).await.unwrap());

        sqlx::query(
            "INSERT INTO players_seen (instance_id, uuid, name, first_seen, last_seen)
             VALUES (1, 'p1', 'Notch', '2026-08-18T11:00:00Z', '2026-08-18T11:30:00Z')",
        )
        .execute(&state.db)
        .await
        .unwrap();
        assert!(
            !instance_was_idle(&state, &schedule).await.unwrap(),
            "someone played since the last run"
        );
    }
}
