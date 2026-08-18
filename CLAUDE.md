# CLAUDE.md — conventions for this repo

Minecraft Server Manager: a cross-platform desktop app (Windows + Linux, both first-class)
for managing multiple local Minecraft servers. See `PLAN.md` for the phase breakdown and the
full data model.

## Stack

- **Tauri v2** — Rust backend, real process control, small binaries. **Never Electron.**
- **React + TypeScript + Vite** frontend.
- **Tailwind CSS + shadcn/ui** for the interface.
- **tokio** for async, **sqlx + SQLite** for persistence.
- **pnpm** as the package manager.

## Architecture rules

1. **All process, filesystem and network work lives in Rust.** The frontend only calls Tauri
   commands and subscribes to events. No business logic in React — no path building, no
   version resolution, no jar URL construction, no file parsing in TypeScript.
2. **Stream, do not poll.** Console output, status changes, metrics and progress are pushed
   with Tauri events. The frontend never sets an interval to ask "is it done yet?".
3. **Every path is a `PathBuf`.** No hardcoded `/` or `\`, no string concatenation of paths,
   no `to_str().unwrap()`. Executable names are chosen with `cfg!(windows)`
   (`java.exe` vs `java`, `run.bat` vs `run.sh`).
4. **Long operations report progress and are cancellable.** Downloads, backups, imports and
   archive operations register a `task_id` with a `CancellationToken` in the task registry,
   emit `task://progress`, and check for cancellation between chunks or entries.
5. **Never block the async runtime with sync I/O.** Use `tokio::fs`, or wrap genuinely
   blocking work (zip, walkdir, `java -version`, registry reads) in `spawn_blocking`.
6. **Errors are typed.** One `AppError` enum built with `thiserror`, `Serialize` for the UI,
   carrying a readable message and a stable `kind` string. **No `unwrap()` or `expect()` in
   command handlers** — and no `panic!` anywhere reachable from a command.

## Domain rules (these have bitten real users; do not "simplify" them away)

### EULA
Never write `eula=true` implicitly. `eula.txt` is only written after an explicit user
acceptance in the UI, and the acceptance is timestamped in the DB (`eula_accepted_at`).

### DB vs `.msm/instance.json`
The **DB row is authoritative** during normal operation. `.msm/instance.json` is written
after every DB mutation as a recovery mirror, but it is only **read** when importing a
folder, or when a folder has no matching DB row (DB loss, manual copy, restored backup).
A stale file never overrides a live DB row.

### Missing / moved instance folders
A gone or moved instance folder is a **recoverable state**, not an error. The instance gets
status `Missing`, stays in the sidebar (greyed, with a "Locate folder…" action that repoints
`path`), and no command panics or auto-deletes anything because of it.

### Orphan processes
Closing the window minimizes to tray and servers keep running, so a crash or reboot can
leave an orphaned JVM holding port 25565. The instance row therefore stores both `pid` and
`process_start_time`; a pid is only trusted when the live process's start time matches
(pids get recycled). On app launch every instance with a pid is reconciled: alive + matching
means status `Unmanaged` ("running, console unavailable") with stop-by-pid still working;
otherwise the pid fields are cleared and the instance is marked `Crashed`.

### Ops / whitelist / bans while running
A running server rewrites `whitelist.json` and `ops.json` from memory on shutdown, so direct
file edits are silently clobbered. All mutations go through a **single gate** (`players::mutate`):
running instance → stdin command (`whitelist add`, `op`, `ban`, …) then re-read the file;
stopped instance → atomic temp-file + rename write. No call site touches those JSON files
directly.

### `server.properties`
The editor preserves comments, key order and unknown keys. Rewriting the file from a typed
struct alone is not acceptable.

### Backups of a running server
`save-off` → `save-all flush` → wait for the save confirmation → archive → `save-on`, and
`save-on` is restored even when the archive step fails or is cancelled.

### `resource_samples` retention
Full resolution for 24 h, downsampled to one row per minute after that, deleted past 30 days.
The prune runs at app start and every 24 h.

## Database

- **sqlx migrations only** (`src-tauri/migrations/NNNN_name.sql`), applied at startup with
  `sqlx::migrate!`. There is no `schema.sql`, and no "drop and recreate" path.
- Times are `TEXT` RFC3339 UTC. Booleans are `INTEGER` 0/1.
- `instances.path` is absolute; every other stored path is relative to the instance directory.
- Console history is **not** stored in SQLite (ring buffer in memory + rotated files under
  `.msm/console/`). Ops/whitelist/ban lists are **not** mirrored into the DB — the server's
  JSON files are the source of truth.

## Rust layout

`src-tauri/src/` is organized by domain: `db/`, `paths.rs`, `instance/`, `download/`,
`providers/`, `java/`, `process/`, `logparse/`, `config/`, `players/`, `worlds/`, `mods/`,
`backup/`, `metrics/`, and `commands/`. Files in `commands/` are **thin wrappers**: they
deserialize arguments, call a domain function, and map errors. Business logic never lives in
a `#[tauri::command]` function.

Event payload types all live in `events.rs`, one struct per event, so the event surface can
be read in one place.

## Frontend layout

`src/features/<domain>/` for feature UI, `src/components/ui/` for shadcn primitives,
`src/lib/ipc.ts` for typed `invoke` wrappers (one function per command, no raw `invoke` calls
elsewhere), `src/lib/events.ts` for typed `listen` helpers. TanStack Query holds
command-derived state; zustand holds pure UI state.

`src/lib/types.ts` is **generated** from the Rust DTOs by `ts-rs` during `cargo test`.
Do not hand-edit it.

## UI conventions

- Dark mode is the default; light mode available; theme persisted in `settings`.
- Sidebar lists instances with a status dot: stopped / starting / running / stopping /
  crashed / unmanaged / missing.
- Instance detail has exactly these tabs: Console, Mods, Config, Players, Worlds, Backups,
  Settings.
- Everything is keyboard-navigable. The console has an autoscroll toggle, search, and copy.
- Destructive actions (delete instance, delete world, restore backup) require an explicit
  confirmation naming the target.

## Testing

- `cargo test` for Rust; fixtures in `src-tauri/tests/fixtures/`; no network in tests
  (HTTP clients sit behind a trait, provider tests use recorded payloads).
- Priority coverage: **version resolution, jar URL building, log parsing**, plus
  `server.properties` round-tripping and path handling.
- Path/OS-specific logic gets tests for both Windows and Linux shapes; parsers accept `\r\n`
  and `\n`.
- Frontend: vitest + testing-library for hooks that shape event streams.

## Definition of done for a phase

`cargo test` green, `cargo clippy -- -D warnings` clean, `pnpm typecheck` clean, a manual
smoke check, then one commit for the phase. Stop for review; do not roll into the next phase.

## Commits

Conventional-commit style subject (`feat:`, `fix:`, `chore:`), imperative mood, and a body
that says what the phase delivered. Commit only when a phase is complete or the user asks.
