//! The scheduled-backup loop.
//!
//! One loop for every instance, not a timer per schedule. Each tick asks which
//! schedules are due, runs them one at a time, and enforces retention straight
//! afterwards.
//!
//! **A missed window runs once, never once per occurrence.** Due-ness is derived
//! from `last_run_at`, so an app that was closed for a week comes back to a
//! single overdue backup per schedule rather than to a queue of 168 of them.

use std::time::Duration;

use chrono::Utc;
use crate::backup::{self, schedule::Schedule, BackupOptions};
use crate::db::record_event;
use crate::error::AppResult;
use crate::state::AppState;

/// How often the loop looks for due schedules. Finer than this buys nothing:
/// the shortest interval a schedule can express is a minute.
const TICK: Duration = Duration::from_secs(60);

/// Why a due schedule did not produce an archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Skip {
    /// Nobody played since the last backup and the schedule asked to skip.
    Idle,
    /// The folder is gone, or the server is live without a console this app owns.
    NotBackable(String),
}

/// Whether a due schedule should actually run now.
pub async fn should_run(state: &AppState, schedule: &Schedule) -> Result<(), Skip> {
    let Ok(row) = crate::instance::get(&state.db, schedule.instance_id).await else {
        return Err(Skip::NotBackable("the instance no longer exists".into()));
    };
    if !row.path_buf().is_dir() {
        return Err(Skip::NotBackable(format!(
            "the folder for \"{}\" is missing",
            row.name
        )));
    }

    // A live server whose console belongs to something else cannot be quiesced,
    // and archiving it regardless would capture a half-written world.
    if state.status_of(&row.uuid).is_live() && !state.supervisor.is_running(&row.uuid) {
        return Err(Skip::NotBackable(format!(
            "\"{}\" is running outside this app, so saving cannot be paused",
            row.name
        )));
    }

    if schedule.skip_if_idle
        && backup::schedule::instance_was_idle(state, schedule)
            .await
            .unwrap_or(false)
    {
        return Err(Skip::Idle);
    }

    Ok(())
}

/// Runs one schedule: backup, retention, and the optional restart.
pub async fn run_one(app: &tauri::AppHandle, state: &AppState, schedule: &Schedule) -> AppResult<()> {
    let now = Utc::now();

    match should_run(state, schedule).await {
        Ok(()) => {}
        Err(Skip::Idle) => {
            // Still counts as a run, otherwise an idle server is asked again
            // every single tick for as long as it stays idle.
            let _ = record_event(
                &state.db,
                schedule.instance_id,
                "backup",
                Some("scheduled backup skipped: nobody played since the last one"),
            )
            .await;
            return backup::schedule::mark_ran(state, schedule, now).await;
        }
        Err(Skip::NotBackable(reason)) => {
            let _ = record_event(
                &state.db,
                schedule.instance_id,
                "error",
                Some(&format!("scheduled backup skipped: {reason}")),
            )
            .await;
            return backup::schedule::mark_ran(state, schedule, now).await;
        }
    }

    let options = BackupOptions {
        format: schedule.format,
        scope: schedule.scope,
        level: schedule.compression_level.and_then(|level| i32::try_from(level).ok()),
        ..BackupOptions::default()
    };

    let (task_id, cancel) = state.tasks.register();
    let result = backup::create(
        state,
        schedule.instance_id,
        options,
        "scheduled",
        Some(schedule.id),
        &cancel,
        |_| {},
    )
    .await;
    state.tasks.finish(&task_id);

    // The run happened, well or badly; recording it is what stops the loop
    // retrying the same overdue window on the next tick.
    backup::schedule::mark_ran(state, schedule, now).await?;

    match result {
        Ok(created) => {
            let _ = record_event(
                &state.db,
                schedule.instance_id,
                "backup",
                Some(&format!("scheduled backup written to {}", created.path)),
            )
            .await;

            let removed = backup::prune(
                state,
                schedule.instance_id,
                schedule.keep_count,
                schedule.keep_days,
            )
            .await
            .unwrap_or(0);
            if removed > 0 {
                let _ = record_event(
                    &state.db,
                    schedule.instance_id,
                    "backup",
                    Some(&format!("retention removed {removed} old backup(s)")),
                )
                .await;
            }

            if let Ok(row) = crate::instance::get(&state.db, schedule.instance_id).await {
                crate::events::backups_changed(app, &row.uuid);

                if schedule.restart_after && state.supervisor.is_running(&row.uuid) {
                    if let Err(err) =
                        crate::process::supervisor::restart(app, state, schedule.instance_id).await
                    {
                        tracing::warn!(error = %err, "restart after a scheduled backup failed");
                    }
                }
            }
        }
        Err(err) => {
            tracing::warn!(error = %err, schedule = schedule.id, "a scheduled backup failed");
            let _ = record_event(
                &state.db,
                schedule.instance_id,
                "error",
                Some(&format!("scheduled backup failed: {err}")),
            )
            .await;
        }
    }

    Ok(())
}

/// Every schedule that is due at `now`.
pub async fn due_now(state: &AppState, now: chrono::DateTime<Utc>) -> AppResult<Vec<Schedule>> {
    Ok(backup::schedule::all_enabled(state)
        .await?
        .into_iter()
        .filter(|schedule| backup::schedule::is_due(schedule, now))
        .collect())
}

/// The loop. Started once at launch; the first tick is what catches up the runs
/// missed while the app was closed.
pub async fn run(app: tauri::AppHandle) {
    use tauri::Manager;

    loop {
        let state = app.state::<AppState>();
        match due_now(&state, Utc::now()).await {
            Ok(schedules) => {
                for schedule in schedules {
                    // Sequentially: two archives of the same disk at once is
                    // slower than one after the other, and a shared save-off
                    // marker cannot be nested.
                    if let Err(err) = run_one(&app, &state, &schedule).await {
                        tracing::warn!(error = %err, "a backup schedule could not be run");
                    }
                }
            }
            Err(err) => tracing::warn!(error = %err, "could not read backup schedules"),
        }

        tokio::time::sleep(TICK).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backup::archive::{Format, Scope};
    use crate::db::now_rfc3339;

    fn schedule(interval: Option<i64>) -> Schedule {
        Schedule {
            id: 1,
            instance_id: 1,
            cron: None,
            interval_minutes: interval,
            scope: Scope::Full,
            format: Format::TarZst,
            compression_level: None,
            keep_count: None,
            keep_days: None,
            enabled: true,
            restart_after: false,
            skip_if_idle: false,
            last_run_at: None,
            next_run_at: None,
        }
    }

    async fn state_with_instance(dir: &std::path::Path) -> AppState {
        let pool = crate::db::connect_in_memory().await.unwrap();
        let state = AppState::new(pool, dir.join("data"));
        let now = now_rfc3339();
        sqlx::query(
            "INSERT INTO instances (uuid, name, path, server_type, mc_version, launch_kind,
                jvm_args, server_args, created_at, updated_at)
             VALUES ('u1', 'Survival', ?, 'paper', '1.21.4', 'jar', '[]', '[]', ?, ?)",
        )
        .bind(dir.join("survival").to_string_lossy().to_string())
        .bind(&now)
        .bind(&now)
        .execute(&state.db)
        .await
        .unwrap();
        state
    }

    #[tokio::test]
    async fn a_week_offline_produces_one_overdue_run_not_one_per_hour() {
        let mut hourly = schedule(Some(60));
        let now = Utc::now();
        hourly.last_run_at = Some((now - chrono::Duration::days(7)).to_rfc3339());

        assert!(backup::schedule::is_due(&hourly, now));

        // Due-ness is a yes/no over the whole gap, so the loop takes exactly one
        // backup and moves the marker to now.
        let after = backup::schedule::next_run(&hourly, Some(now), now + chrono::Duration::minutes(1));
        assert!(
            after.unwrap() > now + chrono::Duration::minutes(1),
            "the next run is an hour out, not a backlog"
        );
    }

    #[tokio::test]
    async fn a_missing_folder_is_a_skip_with_a_reason_not_a_failed_backup() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_with_instance(dir.path()).await;

        let skip = should_run(&state, &schedule(Some(60))).await.unwrap_err();
        assert!(
            matches!(&skip, Skip::NotBackable(reason) if reason.contains("folder")),
            "{skip:?}"
        );
    }

    #[tokio::test]
    async fn a_server_running_outside_this_app_is_skipped_rather_than_torn() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("survival")).unwrap();
        let state = state_with_instance(dir.path()).await;
        state.set_status("u1", crate::db::models::InstanceStatus::Unmanaged);

        let skip = should_run(&state, &schedule(Some(60))).await.unwrap_err();
        assert!(
            matches!(&skip, Skip::NotBackable(reason) if reason.contains("outside this app")),
            "{skip:?}"
        );
    }

    #[tokio::test]
    async fn an_idle_server_is_skipped_only_when_the_schedule_asks() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("survival")).unwrap();
        let state = state_with_instance(dir.path()).await;

        let mut idle_aware = schedule(Some(60));
        idle_aware.skip_if_idle = true;
        idle_aware.last_run_at = Some(now_rfc3339());
        assert_eq!(should_run(&state, &idle_aware).await, Err(Skip::Idle));

        // The same instance with the option off is backed up regardless.
        let mut always = idle_aware.clone();
        always.skip_if_idle = false;
        assert_eq!(should_run(&state, &always).await, Ok(()));

        // A player seen since the last run makes it non-idle again.
        sqlx::query(
            "INSERT INTO players_seen (instance_id, uuid, name, first_seen, last_seen)
             VALUES (1, 'p1', 'Notch', ?, ?)",
        )
        .bind(now_rfc3339())
        .bind(
            (Utc::now() + chrono::Duration::seconds(5))
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        )
        .execute(&state.db)
        .await
        .unwrap();
        assert_eq!(should_run(&state, &idle_aware).await, Ok(()));
    }

    #[tokio::test]
    async fn only_enabled_due_schedules_are_picked_up() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_with_instance(dir.path()).await;

        for (interval, enabled) in [(60, true), (60, false)] {
            sqlx::query(
                "INSERT INTO backup_schedules (instance_id, interval_minutes, scope, format, enabled)
                 VALUES (1, ?, 'full', 'tar_zst', ?)",
            )
            .bind(interval)
            .bind(i64::from(enabled))
            .execute(&state.db)
            .await
            .unwrap();
        }

        let due = due_now(&state, Utc::now()).await.unwrap();
        assert_eq!(due.len(), 1, "the disabled schedule is not due");
        assert!(due[0].enabled);
    }
}
