# Minecraft Server Manager — Plan

A cross-platform (Windows + Linux) desktop app for managing multiple local
Minecraft servers. Tauri v2 (Rust) + React/TypeScript/Vite + Tailwind + shadcn/ui.

---

## 0. Decisions locked in

| Question | Decision |
|---|---|
| Instance storage | **Per-instance arbitrary absolute paths.** Every instance row stores its own `path`. A configurable *default new-instance root* only pre-fills the "create" dialog. |
| Existing servers | **Adopt/import an existing folder** as an instance (detect type/version from jar + files). In scope, Phase 1→2. |
| App close with servers running | **Minimize to tray**, servers keep running. Quit from tray → confirm → graceful stop of all, then exit. |
| RCON | **Deferred** past v1. Player/console control goes through stdin + JSON files. Data model leaves room for it (`rcon_*` columns). |
| CurseForge | **Deferred** past v1. Modrinth only. `mods.source` enum already includes it. |
| `.mrpack` import | **In scope**, Phase 5. |
| Linux verification | `PathBuf` everywhere + cross-platform unit tests + **GitHub Actions matrix (ubuntu-latest, windows-latest)**. You do manual Linux runtime testing. |

Toolchain present on this machine: rustc 1.95, cargo 1.95, node 24.15, pnpm 11.17,
git 2.51, java 26. (Java 26 alone cannot run most MC versions — JDK detection and
per-instance pinning matter; see Phase 2 and §8.)

Package manager: **pnpm**. Repo: **`git init` at the start of Phase 1**, one commit per phase.

---

## 1. Repository layout

```
mc_server_manager/
├─ CLAUDE.md                  # conventions (written before Phase 1 code)
├─ PLAN.md
├─ package.json               # pnpm, vite, react, tailwind
├─ index.html
├─ src/                       # frontend — NO business logic
│  ├─ main.tsx  App.tsx
│  ├─ lib/
│  │  ├─ ipc.ts               # typed invoke() wrappers, one fn per command
│  │  ├─ events.ts            # typed listen() helpers
│  │  └─ types.ts             # mirrors of Rust DTOs (generated, see §6)
│  ├─ components/ui/          # shadcn
│  ├─ features/
│  │  ├─ instances/           # sidebar, create/clone/import dialogs
│  │  ├─ console/  mods/  config/  players/  worlds/  backups/  settings/
│  └─ stores/                 # zustand (UI state) + TanStack Query (command data)
├─ src-tauri/
│  ├─ Cargo.toml  tauri.conf.json  capabilities/
│  ├─ migrations/             # sqlx migrations, 0001_init.sql …
│  └─ src/
│     ├─ main.rs  lib.rs
│     ├─ error.rs             # AppError (thiserror) + Serialize for the UI
│     ├─ state.rs             # AppState: db pool, supervisor, task registry
│     ├─ events.rs            # every event payload type in one place
│     ├─ db/                  # pool, models, queries
│     ├─ paths.rs             # PathBuf builders, sanitization, portability
│     ├─ instance/            # crud, layout, clone, import/adopt
│     ├─ download/            # http, progress, checksum, resume, cache
│     ├─ providers/           # vanilla paper purpur fabric forge neoforge
│     ├─ java/                # detection + version mapping
│     ├─ process/             # supervisor, child, stdin, crash + backoff
│     ├─ logparse/            # log line → structured event
│     ├─ config/              # server.properties typed editor
│     ├─ players/  worlds/
│     ├─ mods/                # modrinth, jar metadata, mrpack
│     ├─ backup/              # archive, schedule, restore, retention
│     ├─ metrics/             # sysinfo sampler
│     └─ commands/            # thin #[tauri::command] wrappers per domain
└─ .github/workflows/ci.yml
```

**Instance folder layout** (inside each instance's own `path`):

```
<instance path>/
├─ server.jar | run.sh / run.bat | libraries/…   (depends on launch kind)
├─ eula.txt  server.properties  ops.json  whitelist.json  banned-*.json
├─ world/  world_nether/  world_the_end/  …
├─ mods/ | plugins/
├─ logs/
└─ .msm/                      # our metadata, never touched by the server
   ├─ instance.json           # mirror of the DB row (survives DB loss / folder move)
   └─ console/*.log           # rotated console capture
```

**Authority rule:** the **DB row is authoritative** during normal operation.
`.msm/instance.json` is written after every DB mutation but is only *read* on import, or
when a folder has no matching DB row (DB loss, manual copy). It never overrides a live row.

---

## 2. Data model (SQLite via sqlx, migrations in `src-tauri/migrations`)

Times are `TEXT` RFC3339 UTC. Booleans are `INTEGER` 0/1. Paths are `TEXT` —
absolute for `instances.path`, otherwise relative to the instance dir.

```sql
-- key/value app settings: theme, default_new_instance_root, max_parallel_downloads,
-- tray_minimize, metrics_retention_hours, curseforge_api_key (post-v1), …
settings(key TEXT PRIMARY KEY, value TEXT NOT NULL)

instances(
  id                INTEGER PRIMARY KEY,
  uuid              TEXT NOT NULL UNIQUE,      -- stable id used in events/.msm
  name              TEXT NOT NULL UNIQUE,
  path              TEXT NOT NULL UNIQUE,      -- absolute, user-chosen
  server_type       TEXT NOT NULL,             -- vanilla|paper|purpur|fabric|forge|neoforge
  mc_version        TEXT NOT NULL,
  loader_version    TEXT,                      -- paper/purpur build, fabric loader, forge/neoforge version
  launch_kind       TEXT NOT NULL,             -- jar | args_file | script
  launch_target     TEXT,                      -- server.jar | @libraries/…/unix_args.txt
  java_path         TEXT,                      -- NULL = auto-select
  java_major        INTEGER,                   -- required/resolved major version
  jvm_args          TEXT NOT NULL,             -- JSON array
  server_args       TEXT NOT NULL,             -- JSON array, e.g. ["--nogui"]
  min_ram_mb        INTEGER NOT NULL,
  max_ram_mb        INTEGER NOT NULL,
  eula_accepted     INTEGER NOT NULL DEFAULT 0,
  eula_accepted_at  TEXT,
  auto_start        INTEGER NOT NULL DEFAULT 0,
  auto_restart      INTEGER NOT NULL DEFAULT 0,
  restart_max       INTEGER NOT NULL DEFAULT 3,   -- attempts inside the window
  restart_window_s  INTEGER NOT NULL DEFAULT 600,
  stop_timeout_s    INTEGER NOT NULL DEFAULT 60,
  rcon_enabled      INTEGER NOT NULL DEFAULT 0,   -- reserved, post-v1
  rcon_port         INTEGER, rcon_password TEXT,  -- reserved, post-v1
  color             TEXT,  notes TEXT,
  last_status       TEXT,  last_exit_code INTEGER,
  last_started_at   TEXT,  last_stopped_at TEXT,
  pid                INTEGER,                    -- OS pid of the running JVM, NULL when stopped
  process_start_time INTEGER,                    -- OS process start time; guards against pid reuse
  created_at        TEXT NOT NULL, updated_at TEXT NOT NULL
)

instance_events(id, instance_id→instances, ts, kind, detail)
  -- kind: started|stopped|crashed|restarted|backup|restore|error
  -- drives the restart-backoff window and the history view

resource_samples(instance_id, ts, cpu_pct REAL, rss_bytes INTEGER)
  -- PK(instance_id, ts); 5 s cadence while running.
  -- Retention: full resolution for 24 h, downsampled to one row per minute after that,
  -- deleted past 30 days. Pruned on app start and once every 24 h thereafter.

java_runtimes(id, path UNIQUE, major INTEGER, vendor, arch, source, valid, detected_at)
  -- source: path|java_home|registry|common_dir|manual

mods(
  id, instance_id→instances, target_dir TEXT,        -- mods|plugins
  file_name TEXT NOT NULL,                           -- on-disk name without .disabled
  display_name, version, loader, mc_version,
  source TEXT NOT NULL,                              -- modrinth|curseforge|local
  project_id, version_id, page_url,
  sha1, sha512, size_bytes,
  enabled INTEGER NOT NULL DEFAULT 1,                -- .jar.disabled rename
  pinned  INTEGER NOT NULL DEFAULT 0,
  update_version_id TEXT,                            -- newest seen, if any
  installed_at, updated_at,
  UNIQUE(instance_id, target_dir, file_name)
)
mod_dependencies(mod_id→mods, dep_project_id, dep_version_id, required INTEGER)

players_seen(instance_id, uuid, name, first_seen, last_seen,
             PRIMARY KEY(instance_id, uuid))
  -- history from log parsing; ops/whitelist/bans stay in the server's JSON files
  -- (those files are the source of truth, never mirrored into the DB)

backups(id, instance_id, path, format, scope, kind, label, size_bytes, sha256, created_at)
  -- format: zip|tar.zst   scope: full|worlds   kind: manual|scheduled|pre_restore
backup_schedules(id, instance_id, cron TEXT, scope, format, keep_count, keep_days,
                 enabled, last_run_at, next_run_at)

artifact_cache(url TEXT PRIMARY KEY, sha1, sha256, path, size_bytes, fetched_at)
  -- shared jar/installer cache: cloning an instance re-downloads nothing
```

**Runtime-only state (never persisted):** `InstanceStatus = Stopped | Starting | Running |
Stopping | Crashed | Unmanaged | Missing`, owned by the supervisor and pushed to the UI by
event. `last_status` is only a hint for the first paint after app launch.

- `Unmanaged` — the process is alive but this app instance does not own its stdio
  ("running, console unavailable"); stop still works, via pid.
- `Missing` — the instance folder is gone or moved. Recoverable state, never an error:
  the UI greys the instance and offers **Locate folder…**, which repoints `path`.

### Orphan recovery (close-to-tray consequence)

Because closing to tray keeps servers alive, a crash or reboot can leave an orphaned JVM
holding port 25565. On every app launch, each instance with a non-NULL `pid` is reconciled:

1. Look the pid up (`sysinfo`) and compare its **process start time** against
   `process_start_time` — equal means it really is our process, not a recycled pid.
2. Alive and matching → status `Unmanaged` (Phase 3 attempts re-attach where possible);
   stop/kill act on the pid.
3. Not alive → `pid`/`process_start_time` cleared, status `Crashed`, `instance_events` row written.

**Console history is not in SQLite:** an in-memory ring buffer (default 5 000 lines) per
instance, plus rotated files under `.msm/console/`.

---

## 3. Rust ↔ UI contract

**Commands** (all `async`, all `Result<T, AppError>`, no `unwrap()` in handlers):

- instances: `instance_list/get/create/clone/rename/delete/import_existing/update_settings`
- setup: `eula_get/eula_accept`, `version_list(server_type)`, `install_server(...)`
- java: `java_list/java_rescan/java_validate`
- process: `instance_start/stop/kill/restart/send_command`, `console_tail(uuid, n)`
- config: `props_read/props_write/props_schema`
- players: `players_read/players_write` (ops, whitelist, banned players/IPs)
- worlds: `worlds_list/switch/delete/import/export`
- mods: `mods_list/search/install/remove/toggle/pin/check_updates/install_local/import_mrpack`
- backups: `backup_create/list/delete/restore`, `schedule_upsert/list/delete`
- misc: `metrics_range(uuid, from, to)`, `task_cancel(task_id)`

**Events** (Tauri emit — never polling):

- `instance://status` `{uuid, status, exit_code?}`
- `instance://console` `{uuid, lines: [ParsedLine]}` — batched ~100 ms to avoid IPC storms
- `instance://player` `{uuid, event: join|leave, player}`
- `instance://metrics` `{uuid, ts, cpu_pct, rss_bytes}`
- `task://progress` `{task_id, kind, phase, done, total, msg}` · `task://done` `{task_id, result}`

Every long operation registers a `task_id` in a task registry holding a `CancellationToken`;
`task_cancel` fires it and downloads/backups check it between chunks/entries.

---

## 4. Phases

Each phase ends with: tests passing, `cargo clippy -D warnings` and `tsc --noEmit` clean,
a manual smoke check on Windows, and a commit. I stop for your review after each one.

### Phase 1 — Scaffold, DB, instance CRUD, folder layout
- `git init`; pnpm + Vite + React + TS + Tailwind + shadcn; Tauri v2 scaffold with capabilities.
- `AppError` (thiserror) + serialized error surface; tracing to a rotating log file.
- sqlx SQLite pool + migration `0001_init.sql` (full schema above); DB in the platform config dir.
- Instance CRUD: create (folder scaffold + `.msm/instance.json`), clone (copy with excludes,
  new uuid/name/path), rename, delete (with an explicit "also delete files?" choice).
- **Import existing folder:** scan for `server.jar` / `paper-*.jar` / `libraries/` / `run.sh`,
  read `server.properties` and `version_history.json`, guess type + version, user confirms.
- **Missing-folder handling:** every read of an instance verifies its `path`; a gone/moved
  folder yields status `Missing` plus a **Locate folder…** action that repoints `path`.
  Never an error toast, never a panic, never an auto-delete.
- **Orphan reconciliation on launch** (see §2): pid + `process_start_time` check,
  `Unmanaged` or `Crashed`, plus a stop-by-pid path so an orphan can be killed in Phase 1.
- **Retention prune** for `resource_samples` (24 h full / per-minute to 30 d / delete beyond),
  run at startup and every 24 h.
- Sidebar with status dots + instance detail shell with the seven tabs (stubs), dark default /
  light theme, tray icon, close-to-tray and confirm-on-quit wiring.
- Tests: path builders, name→folder sanitization, clone exclude rules, import detection,
  pid/start-time reconciliation, retention-prune SQL.

### Phase 2 — Server jar downloads (all six types) + Java detection
- `ServerProvider` trait: `list_versions()`, `resolve(version, loader_version) -> Artifact
  {url, sha1?, sha256?, kind}`, `install(dir, artifact) -> LaunchSpec`.
- Vanilla (version_manifest_v2 → per-version JSON → `downloads.server`, SHA-1),
  Paper (**`fill.papermc.io/v3`** — the v2 API in the original brief has been sunset and now
  answers `{"ok":false,"error":"sunset"}`; v3 builds carry SHA-256),
  Purpur (`/v2/purpur`, MD5 where provided),
  Fabric (meta v2 → server launcher jar), NeoForge (`maven-metadata.xml` → installer),
  Forge (promotions + installer, run headlessly with `--installServer`).
- Forge/NeoForge ≥ 1.17 produce `libraries/` + `@…/win_args.txt` / `unix_args.txt`
  → `launch_kind = args_file`; older ones stay `jar`.
- Download engine: reqwest streaming, progress events, cancel, resume, checksum verify,
  `artifact_cache` reuse.
- EULA: read/write `eula.txt` only after an explicit UI accept, timestamped in the DB.
  Never written implicitly.
- Java detection: PATH, `JAVA_HOME`, common dirs (Program Files/Eclipse Adoptium|Java|Microsoft,
  `/usr/lib/jvm`, SDKMAN, Homebrew), Windows registry (`winreg`); parse `java -version`;
  Java requirement read from Mojang's `javaVersion.majorVersion` where available, with the
  table (26.x → 25, 1.20.5+ → 21, 1.17–1.20.4 → 17, older → 8) as the offline fallback;
  mismatch warning + per-instance pin; JVM arg defaults editable.
- Tests: **version resolution + jar URL building for all six providers** (recorded fixtures,
  no network), Java version-string parsing, MC→Java mapping table.

### Phase 3 — Process control + console
- Supervisor: `HashMap<uuid, RunningInstance>`, spawn via `tokio::process::Command`
  (`kill_on_drop`, job object on Windows / process group on Linux so the JVM cannot orphan).
- Start: argv from `LaunchSpec` + JVM args + heap flags; working dir = instance path.
- Stop: `stop\n` to stdin → wait `stop_timeout_s` → terminate → kill. Restart = stop + start.
- stdout/stderr framed into the ring buffer and batched into `instance://console`; stdin sends.
- `logparse`: timestamp, level, thread, message; recognizes "Done (…)! For help…",
  player join/leave (with UUID line), "Stopping server", crash/exception blocks, port-in-use.
- Crash detection (non-zero exit or crash pattern) → auto-restart with exponential backoff,
  capped by `restart_max` within `restart_window_s`; every transition logged to `instance_events`.
- Console UI: virtualized list, level colors, autoscroll toggle, search/filter, copy,
  command input with history.
- Tests: log parser over captured vanilla/paper/fabric/forge log fixtures, both line endings.

### Phase 4 — `server.properties`, players, worlds
- Properties reader/writer that **preserves comments, ordering and unknown keys**; typed schema
  (bool/int/enum/string/range) with descriptions and search in the UI; cross-instance
  port-conflict warning.
- Players: `ops.json`, `whitelist.json`, `banned-players.json`, `banned-ips.json`.
  **A running server rewrites these files from memory on shutdown, so direct edits get
  clobbered.** One gate — `players::mutate()` — decides per mutation: instance running →
  stdin command (`whitelist add/remove`, `op`/`deop`, `ban`/`pardon`, `ban-ip`/`pardon-ip`)
  then re-read the file; instance stopped → atomic temp + rename write. No call site
  touches these files directly. Mojang profile lookup for name → UUID when adding offline.
- Worlds: enumerate level dirs (`level.dat` present), size, last played; switch via `level-name`;
  delete with confirmation; import/export zip with progress + cancel.
- Tests: properties round-trip incl. comment preservation, atomic JSON write, world detection.

### Phase 5 — Modrinth + local jar management
- Modrinth v2 client (search with loader + game-version facets, project, versions, dependencies),
  descriptive User-Agent, rate-limit handling.
- Install into `mods/` or `plugins/` depending on server type, hash-verified; dependency
  resolution behind a confirm dialog; enable/disable via `.jar.disabled` rename; version pinning;
  bulk update check.
- Local jar drag-and-drop: read `fabric.mod.json`, `META-INF/mods.toml` /
  `META-INF/neoforge.mods.toml`, `plugin.yml`, `paper-plugin.yml` from the zip for
  name/version/loader.
- `.mrpack` import: parse `modrinth.index.json`, download `files[]` with hash verification,
  apply `overrides/` and `server-overrides/`, create or update the instance.
- Tests: jar metadata parsing fixtures, mrpack index parsing, facet/query building.

### Phase 6 — Backups, scheduling, resource graphs
- Archive engine: zip and tar.zst, streaming with progress + cancel, scope full|worlds,
  exclusions (`.msm`, logs, caches).
- If the server is running: `save-off` → `save-all flush` → wait for the save confirmation →
  archive → `save-on`, with `save-on` restored even on error or cancel.
- Retention by count and/or age; scheduler task (cron) that survives restarts via `next_run_at`.
- Restore: confirmation step + automatic `pre_restore` backup, then replace.
- Metrics: `sysinfo` sampler per running instance (5 s), retention trim, CPU/RAM charts.
- Tests: retention selection, cron next-run computation, archive round-trip in a temp dir.

### Phase 7 — Polish + packaging
- Error/empty/loading states everywhere, toast surface for `AppError`, keyboard-navigation pass,
  focus rings, a11y labels, first-run experience.
- Packaging: `.msi` (WiX) + `.deb` + AppImage via `tauri build`; icons and app metadata.
- CI: GitHub Actions matrix (ubuntu-latest + windows-latest) → `cargo test`, `cargo clippy`,
  `pnpm test`, `tsc`, plus a release job that produces installers.
- README with build and run instructions.

---

## 5. Cross-platform rules (enforced from Phase 1)

- Every path built with `PathBuf` / `Path::join`; no `/` or `\` literals; no `to_str().unwrap()`.
- Executable names chosen with `cfg!(windows)` (`java.exe` vs `java`, `run.bat` vs `run.sh`).
- Windows: `CREATE_NO_WINDOW` for spawned Java, job object so children die with the app.
- Linux: process group + SIGTERM; set the executable bit on `run.sh` after a Forge install.
- Line endings: parsers accept `\r\n` and `\n`; files are written with `\n`.
- Case sensitivity: never rely on case-insensitive filename matching.
- Instance name → folder name is validated for reserved/invalid characters on both OSes.

## 6. Types across the boundary

Rust DTOs derive `serde::Serialize` plus `ts-rs` (`#[ts(export)]`), so `src/lib/types.ts` is
**generated by `cargo test`** rather than hand-maintained; `pnpm typecheck` then catches drift.

## 7. Testing strategy

- Rust unit tests colocated (`#[cfg(test)] mod tests`); fixtures in `src-tauri/tests/fixtures/`.
- Priority per your step 4: **version resolution, jar URL building, log parsing** — all pure
  functions over recorded API payloads, zero network in tests.
- Integration tests over `tempfile` dirs for instance layout, backups, properties round-trip.
- Frontend: vitest + testing-library for the hooks/reducers that shape event streams.
- Network clients sit behind a small `HttpClient` trait so provider tests use fixtures.

## 8. Risks I will flag as I hit them

1. **Forge headless install** is the flakiest step (installer layout varies by version, needs a
   JDK). Fallback: keep the installer output and surface its log in the UI on failure.
2. **Java 26 is the only JDK on this machine** — most MC versions want 8/17/21. Phase 2 will warn,
   but installing Temurin 21 would let Phase 3 be smoke-tested for real.
3. **Windows child cleanup** — a killed launcher can orphan the JVM; job objects fix it and
   deserve an explicit test.
4. **Modrinth rate limits** on bulk update checks → batched version queries + caching.
5. AppImage/`.deb` can only be truly verified by you on real Linux (see §0).

---

## 9. Post-v1 backlog (explicitly out of scope now)

RCON control channel · CurseForge integration · app auto-update · remote/SSH servers ·
tunneling (playit/ngrok) · plugin config editors · multi-user permissions · world map rendering.
