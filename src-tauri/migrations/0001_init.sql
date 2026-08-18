-- Initial schema. See PLAN.md §2 for the rationale behind each table.
-- Times are TEXT RFC3339 UTC. Booleans are INTEGER 0/1.

PRAGMA foreign_keys = ON;

CREATE TABLE settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE instances (
    id                 INTEGER PRIMARY KEY,
    uuid               TEXT    NOT NULL UNIQUE,
    name               TEXT    NOT NULL UNIQUE,
    path               TEXT    NOT NULL UNIQUE,
    server_type        TEXT    NOT NULL,
    mc_version         TEXT    NOT NULL,
    loader_version     TEXT,
    launch_kind        TEXT    NOT NULL DEFAULT 'jar',
    launch_target      TEXT,
    java_path          TEXT,
    java_major         INTEGER,
    jvm_args           TEXT    NOT NULL DEFAULT '[]',
    server_args        TEXT    NOT NULL DEFAULT '["--nogui"]',
    min_ram_mb         INTEGER NOT NULL DEFAULT 1024,
    max_ram_mb         INTEGER NOT NULL DEFAULT 4096,
    eula_accepted      INTEGER NOT NULL DEFAULT 0,
    eula_accepted_at   TEXT,
    auto_start         INTEGER NOT NULL DEFAULT 0,
    auto_restart       INTEGER NOT NULL DEFAULT 0,
    restart_max        INTEGER NOT NULL DEFAULT 3,
    restart_window_s   INTEGER NOT NULL DEFAULT 600,
    stop_timeout_s     INTEGER NOT NULL DEFAULT 60,
    rcon_enabled       INTEGER NOT NULL DEFAULT 0,
    rcon_port          INTEGER,
    rcon_password      TEXT,
    color              TEXT,
    notes              TEXT,
    last_status        TEXT,
    last_exit_code     INTEGER,
    last_started_at    TEXT,
    last_stopped_at    TEXT,
    -- pid is only trusted together with process_start_time: pids get recycled.
    pid                INTEGER,
    process_start_time INTEGER,
    created_at         TEXT    NOT NULL,
    updated_at         TEXT    NOT NULL
);

CREATE TABLE instance_events (
    id          INTEGER PRIMARY KEY,
    instance_id INTEGER NOT NULL REFERENCES instances(id) ON DELETE CASCADE,
    ts          TEXT    NOT NULL,
    kind        TEXT    NOT NULL, -- started|stopped|crashed|restarted|backup|restore|error|imported|created
    detail      TEXT
);
CREATE INDEX idx_instance_events_instance_ts ON instance_events(instance_id, ts);

CREATE TABLE resource_samples (
    instance_id INTEGER NOT NULL REFERENCES instances(id) ON DELETE CASCADE,
    ts          TEXT    NOT NULL,
    cpu_pct     REAL    NOT NULL,
    rss_bytes   INTEGER NOT NULL,
    PRIMARY KEY (instance_id, ts)
);

CREATE TABLE java_runtimes (
    id          INTEGER PRIMARY KEY,
    path        TEXT    NOT NULL UNIQUE,
    major       INTEGER NOT NULL,
    vendor      TEXT,
    arch        TEXT,
    source      TEXT    NOT NULL, -- path|java_home|registry|common_dir|manual
    valid       INTEGER NOT NULL DEFAULT 1,
    detected_at TEXT    NOT NULL
);

CREATE TABLE mods (
    id                INTEGER PRIMARY KEY,
    instance_id       INTEGER NOT NULL REFERENCES instances(id) ON DELETE CASCADE,
    target_dir        TEXT    NOT NULL, -- mods|plugins
    file_name         TEXT    NOT NULL, -- on-disk name without the .disabled suffix
    display_name      TEXT,
    version           TEXT,
    loader            TEXT,
    mc_version        TEXT,
    source            TEXT    NOT NULL, -- modrinth|curseforge|local
    project_id        TEXT,
    version_id        TEXT,
    page_url          TEXT,
    sha1              TEXT,
    sha512            TEXT,
    size_bytes        INTEGER,
    enabled           INTEGER NOT NULL DEFAULT 1,
    pinned            INTEGER NOT NULL DEFAULT 0,
    update_version_id TEXT,
    installed_at      TEXT    NOT NULL,
    updated_at        TEXT    NOT NULL,
    UNIQUE (instance_id, target_dir, file_name)
);

CREATE TABLE mod_dependencies (
    mod_id         INTEGER NOT NULL REFERENCES mods(id) ON DELETE CASCADE,
    dep_project_id TEXT    NOT NULL,
    dep_version_id TEXT,
    required       INTEGER NOT NULL DEFAULT 1,
    PRIMARY KEY (mod_id, dep_project_id)
);

-- Join/leave history only. ops.json / whitelist.json / banned-*.json stay the
-- source of truth for permissions and are never mirrored here.
CREATE TABLE players_seen (
    instance_id INTEGER NOT NULL REFERENCES instances(id) ON DELETE CASCADE,
    uuid        TEXT    NOT NULL,
    name        TEXT    NOT NULL,
    first_seen  TEXT    NOT NULL,
    last_seen   TEXT    NOT NULL,
    PRIMARY KEY (instance_id, uuid)
);

CREATE TABLE backups (
    id          INTEGER PRIMARY KEY,
    instance_id INTEGER NOT NULL REFERENCES instances(id) ON DELETE CASCADE,
    path        TEXT    NOT NULL,
    format      TEXT    NOT NULL, -- zip|tar.zst
    scope       TEXT    NOT NULL, -- full|worlds
    kind        TEXT    NOT NULL, -- manual|scheduled|pre_restore
    label       TEXT,
    size_bytes  INTEGER NOT NULL,
    sha256      TEXT,
    created_at  TEXT    NOT NULL
);
CREATE INDEX idx_backups_instance ON backups(instance_id, created_at);

CREATE TABLE backup_schedules (
    id          INTEGER PRIMARY KEY,
    instance_id INTEGER NOT NULL REFERENCES instances(id) ON DELETE CASCADE,
    cron        TEXT    NOT NULL,
    scope       TEXT    NOT NULL DEFAULT 'full',
    format      TEXT    NOT NULL DEFAULT 'zip',
    keep_count  INTEGER,
    keep_days   INTEGER,
    enabled     INTEGER NOT NULL DEFAULT 1,
    last_run_at TEXT,
    next_run_at TEXT
);

CREATE TABLE artifact_cache (
    url        TEXT PRIMARY KEY,
    sha1       TEXT,
    sha256     TEXT,
    path       TEXT    NOT NULL,
    size_bytes INTEGER NOT NULL,
    fetched_at TEXT    NOT NULL
);
