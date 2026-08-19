-- Phase 6: backup options, scheduling, and metrics that include player counts.

-- Player count is sampled alongside CPU and memory, so the charts can line up
-- load against who was online.
ALTER TABLE resource_samples ADD COLUMN players INTEGER;

-- Set while a backup has told a running server to stop saving. If the app dies
-- mid-backup the marker survives, and saving is re-enabled the next time the
-- server is under this app's control.
ALTER TABLE instances ADD COLUMN saving_disabled_at TEXT;

-- Schedules gain the options the UI offers, and `cron` becomes optional: an
-- interval schedule has no time of day. SQLite cannot relax a NOT NULL in
-- place, so the table is rebuilt and its rows copied across.
CREATE TABLE backup_schedules_new (
    id                INTEGER PRIMARY KEY,
    instance_id       INTEGER NOT NULL REFERENCES instances(id) ON DELETE CASCADE,
    -- Daily time as HH:MM, or NULL for an interval schedule.
    cron              TEXT,
    interval_minutes  INTEGER,
    scope             TEXT    NOT NULL DEFAULT 'full',
    format            TEXT    NOT NULL DEFAULT 'tar_zst',
    compression_level INTEGER,
    keep_count        INTEGER,
    keep_days         INTEGER,
    enabled           INTEGER NOT NULL DEFAULT 1,
    restart_after     INTEGER NOT NULL DEFAULT 0,
    -- Skips a run when nobody has been online since the last one: an idle
    -- server otherwise accumulates identical archives.
    skip_if_idle      INTEGER NOT NULL DEFAULT 0,
    last_run_at       TEXT,
    next_run_at       TEXT
);

INSERT INTO backup_schedules_new
    (id, instance_id, cron, scope, format, keep_count, keep_days, enabled, last_run_at, next_run_at)
SELECT id, instance_id, cron, scope, format, keep_count, keep_days, enabled, last_run_at, next_run_at
FROM backup_schedules;

DROP TABLE backup_schedules;
ALTER TABLE backup_schedules_new RENAME TO backup_schedules;

-- Backups remember how they were made, so a restore knows what it is reading.
ALTER TABLE backups ADD COLUMN compression_level INTEGER;
ALTER TABLE backups ADD COLUMN schedule_id INTEGER REFERENCES backup_schedules(id) ON DELETE SET NULL;
