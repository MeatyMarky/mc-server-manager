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

### Minecraft versions come in two eras
`1.MINOR[.PATCH]` and the calendar line (`26.1`, `26.1.2`, `26.2`). Anything that parses,
compares or sorts a version goes through `mcversion.rs` — a hardcoded "starts with 1." check
is a bug. NeoForge encodes the target version in its own number and changed encoding with the
era too (`21.1.65` is 1.21.1; `26.2.0.62` is 26.2).

### Version *ordering* is release chronology, never string parsing
The two numbering schemes are not comparable as numbers, so every user-visible sort —
version pickers, "is there a newer build", mod filtering — goes through
`mcversion::VersionIndex`, built from Mojang's manifest (`releaseTime` plus manifest
position) and cached in `mc_version_index`. Component ordering
(`sort_newest_first_by_components`) exists only as the fallback for ids the manifest does
not list, and those always sort after ids it does.

### Java requirements come from Mojang, not from a table
The per-version JSON at `piston-meta` states `javaVersion.majorVersion`, and that is what an
install records. The table in `java/version.rs` is only the offline fallback (26.x needs
Java 25, 1.20.5+ needs 21, 1.17+ needs 17, older needs 8).

### A 32-bit JVM is never chosen, and never launched with a big heap
`java -version` prints "64-Bit Server VM" for a 64-bit build; the absence of that
marker means 32-bit, and `java_runtimes.bits` records which. 32-bit runtimes are excluded
from automatic selection entirely (`JavaRuntime::usable_for_servers`) and shown greyed with
"32-bit, not suitable for servers" — they stay in the list, because a user who browses to
one deserves an explanation rather than a disappearance. A row with no recorded width is
treated the same and re-probed on the next scan; assuming 64-bit is what let a
`Program Files (x86)` Java 8 be picked in the first place.

Preflight then refuses a launch the JVM would reject anyway: 32-bit plus an effective heap
above `MAX_HEAP_32BIT_MB` (1500) fails with the binary's path, its width and the way out,
because the JVM's own "Invalid maximum heap size: -Xmx8192M" never says which Java produced
it. **The heap comes from `launch::effective_heap_mb`**, which resolves it the way `plan`
builds the command line — RAM fields first, a custom `-Xmx` appended after and therefore
winning — so an instance whose 8 GB lives in the RAM field is checked like any other. A
script launch resolves neither: it runs `java` from `PATH` with the heap in
`user_jvm_args.txt`, so both are read (`detect::java_on_path`, `launch::script_heap_mb`) and
checked the same way.

### The spawned command line is logged verbatim, before anything else
`launch::quoted_command` renders the program and every argument quoted, with control
characters escaped, and that line goes to the log at info level and into the instance
console before the process is spawned — along with the resolved binary, its recorded width
and the effective heap. A JVM's own complaint names a flag but never what was passed to it,
and `-Xmx8192M` and `-Xmx8192M` look identical in a console, so this is the first thing
to read when a start fails.

`launch::validate_args` then refuses a command line the JVM would only reject after
spawning: a heap flag that is not digits plus an optional unit, or more than one `-Xmx`.
The RAM fields and a custom `-Xmx` no longer both reach the command line — a custom flag
replaces the generated one in `heap_args_resolved`, so exactly one setting exists and it is
the one the user set. Arguments read out of a file go through `launch::args_file_tokens`,
which trims carriage returns and blank lines, because the Forge and NeoForge installers
write those files with CRLF endings.

### A start that never finished is not a crash
Auto-restart exists for a server that was running and died. A process that exits before the
"Done" line failed to start — a bad heap, a missing jar, a taken port — and retrying repeats
a deterministic failure four times over while burying the real message. `backoff::classify`
sorts exits into requested / failed-start / crash from whether `reached_ready` was ever set,
failed starts are recorded as `failed_start` (so they never consume the crash budget), and
the last console line is repeated with the reason instead of scrolling away.

### Downloads
Bytes land in `<file>.part`, resume with a `Range` request when the server allows it, are
checksum-verified before the rename, and only then take the final name. A half-downloaded
file must never be mistaken for a complete one, so nothing else writes to the final path.

### Installer failures
Forge and NeoForge installers run inside `.msm/staging`. On failure the staging folder is
deleted (no half-written `libraries/`), the full transcript is kept at `.msm/installer-*.log`,
and the error carries the log path plus its tail so the UI can show what the installer said
instead of a generic message.

### Stopping a server has stages
`stop` on stdin → wait `stop_timeout_s` → SIGTERM (`taskkill /T` on Windows) → wait → SIGKILL
(`taskkill /F`). The stage actually reached is returned and shown, so "stopped cleanly" and
"had to be killed" never read the same. Servers are started in their own process group so a
stop reaches the JVM's children.

### Console output is batched, never per line
A server generating chunks prints thousands of lines a second. Lines go into a bounded ring
buffer (5 000) plus rotated files under `.msm/console/`, and are emitted as one
`instance://console` event per 100 ms or per 250 lines. The frontend coalesces those batches
on an animation frame. One event per line would lock the UI.

### Restarting after a crash is capped
Auto-restart uses exponential backoff (5 s, 10 s, 20 s… capped at 5 min) and stops entirely
after `restart_max` crashes inside `restart_window_s`, so a server that dies on boot cannot
spin. Every attempt and every give-up is written to `instance_events`.

### Log formats differ per family
Vanilla, Paper, Forge (log4j plus a logger bracket) and Fabric (a parenthesised logger) all
print differently, and `logparse` handles each; unparsable lines are still shown verbatim.
Test any parser change against the recorded samples in `tests/fixtures/log_*.txt`.

### `server.properties`
It is a **Java properties file**, not `key=value` lines: `:` separates too, whitespace can
separate, backslashes escape, `\uXXXX` encodes any character, and a trailing backslash
continues onto the next line. `config::properties` keeps every physical line as it was read,
so an untouched file round-trips byte for byte and comments, ordering and the keys plugins
invent all survive. Only the edited line is rewritten. Writes are atomic, and the file as it
was before this app first touched it is kept once as `server.properties.orig`.

Encoding is UTF-8 both ways (Minecraft reads and writes UTF-8), with a Latin-1 fallback for
files from older servers; escaped `\uXXXX` input is decoded on read. A non-ASCII MOTD has to
survive read, edit, write *and* the server rewriting the file on boot — there is a live test
for exactly that.

### Mods come from a source behind a trait
`mods::source::ModSource` is the boundary: search, project, versions. Modrinth is the one
implementation, and no Modrinth-shaped type, id format or facet string may appear outside
`mods/modrinth.rs` — CurseForge has to be a second implementation, never a second code path.
Modrinth also requires an identifying User-Agent (project, version, contact URL) and
publishes a request budget in headers; `mods::ratelimit` holds one limiter per host for the
whole app and backs off on 429 rather than retrying blind.

### The install target comes from the server type, never from the jar
Paper and Purpur load `plugins/`, Fabric/Forge/NeoForge load `mods/`, vanilla loads neither
and is refused with a sentence explaining what to install instead. A jar that declares a
different loader or Minecraft version is a **warning**, not a refusal: declarations are
often conservative and refusing would be wrong more often than warning.

### Dependency resolution is confirmed before anything downloads
Required dependencies are followed recursively into a plan the user confirms; optional ones
are listed and never installed on their own; two versions of one project is a conflict that
is refused by name. Everything installed is recorded in `mods`/`mod_dependencies` so an
uninstall can say what depended on it.

### `.mrpack` import
The `env` field decides what a server gets: a file marked `unsupported` on the server side is
skipped entirely, because a client-only mod on a server is a guaranteed crash. Download URLs
must be on Modrinth's allowlisted hosts and every file must carry a SHA-512, or the pack is
refused with the offending file named. The import is staged in `.msm/pack-staging`, verified,
and only then committed, so a failed import leaves no half-populated `mods/`.

### Worlds
A world is any folder holding a `level.dat`; its metadata comes from the NBT, gzipped or not.
Sizing walks every region file, so it runs in `spawn_blocking` behind a task id with progress
and cancellation, never on the runtime. Switching worlds rewrites `level-name` and is refused
while the server runs, as is deleting. Zip entry paths are sanitised on import: an archive
naming `../../etc/passwd` must never write outside the instance folder.

### Backups of a running server
`save-off` → `save-all flush` → wait for the save confirmation → archive → `save-on`, and
`save-on` is restored even when the archive step fails or is cancelled. Saving being off is
recorded in the DB (`instances.saving_disabled_at`) **before** `save-off` is sent, so an app
that dies mid-backup can put it right: at launch a marked instance whose process did not
survive has the marker cleared (a stopped JVM has no `save-off` state to undo), while one that
outlived the app keeps it until a console exists again, and `save-on` is sent the moment the
server reports ready. A server that is live but whose console this app does not own cannot be
quiesced, so a backup of it is refused rather than written torn.

Archives land under `<data>/backups/<instance-uuid>/`, are written as `.part` and renamed, and
take a suffixed name if one already exists — a restore takes its safety backup in the same
second as the archive it is reading, and second-resolution names would otherwise collide.

### Retention keeps a backup if *either* rule wants it
"Keep 5" and "keep 7 days" together mean an archive survives while either limit still claims it,
never the intersection. Manual and `pre_restore` backups are never pruned automatically — the
safety copy taken before a restore is the one a user reaches for when the restore was the mistake.

### A missed schedule runs once, not once per occurrence
Due-ness is derived from `last_run_at`, so an app closed for a week comes back to a single
overdue backup per schedule. Anything that skips a due run (nobody played, folder missing) still
marks it as run, or the loop asks again every tick for as long as the condition holds.

### Errors are written for the person reading them
`AppError` carries two texts. `user_message()` is one plain sentence with no Rust in it,
`hint()` is the next thing to do, and the `Display` text travels alongside as `technical`
behind a "details" expander. A new variant adds arms to all three, and the test in
`error.rs` walks every variant to check the readable one is a sentence and leaks no
`sqlx`/`reqwest`/`os error` noise. Failures a user can act on get their own variant rather
than `Other` — no Java, Java too old, port in use, disk full, offline, rate limited, EULA
not accepted, corrupt instance — because the UI branches on `kind` to offer the fix, and it
cannot branch on prose.

### `resource_samples` retention
Full resolution for 24 h, downsampled to one row per minute after that, deleted past 30 days.
The prune runs at app start and every 24 h.

### One metrics collector, never one task per server
A single loop refreshes the process table once per tick and writes a row per running instance,
so the sampling cost is the same for one server and for twenty. Charts read the tier their
window has: full resolution inside an hour, minute buckets for a day, ten-minute and hourly
buckets beyond that — asking for finer detail than retention kept would only invent it.

### The problem report never leaves on its own
`diag::preview` returns the exact text of every part; the dialog shows all of it, and only
then does `write_zip` put it where the user chose. Nothing uploads, and the preview carries
the notice that paths hold their user name and consoles hold player chat. The zip is rebuilt
from the backend at write time rather than sent back from the UI.

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

- `cargo test` for Rust; fixtures in `src-tauri/tests/fixtures/`; **no network in tests**.
  Providers are generic over the `Fetch` trait and tests drive them with `FixtureFetch`, which
  maps a URL to a recorded payload and fails on any URL a test did not record.
- Live-API checks live in `tests/network_smoke.rs` and are all `#[ignore]`. Run them by hand
  after touching providers, the downloader or the installer:
  `cargo test --test network_smoke -- --ignored --nocapture`.
- **Stale fixtures are maintenance, not a break.** The recordings pin real builds that the
  upstream APIs eventually retire. Re-record them with one command:

  ```bash
  cargo xtask refresh-fixtures
  ```

  Then review the diff and re-run `cargo test`. Tests must assert on *shapes and
  relationships* (newest-first ordering, checksum length, URL construction), never on a
  specific build number — a test that breaks purely because Paper shipped a new build is a
  badly written test, so fix the test rather than pinning the fixture.
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
