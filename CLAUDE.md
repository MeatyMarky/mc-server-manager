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

### Java detection: probe everything, trust no path, and do not trust an old scan
Candidates come from `JAVA_HOME`, `PATH`, `CLASSPATH`, the registry, and the install roots —
including Oracle's two shim folders (`Common Files\Oracle\Java\javapath` for the current
JDK, `java8path` for the Java 8 line) and the per-user roots under
`%LOCALAPPDATA%\Programs`. Both shims are called `java.exe`, they are different JVMs of
different widths, and **nothing about a path says which** — `detect::resolve_shim`
canonicalizes only to deduplicate, never to judge. The fixture pair in `java/version.rs`
exists to keep the "read the width out of the path" shortcut from coming back.

A cached scan older than `CACHE_MAX_AGE_HOURS` (24) is redone at launch
(`java::rescan_if_stale`), and Settings shows when the list was built. A JDK installed after
the last scan is otherwise invisible while the picker looks complete.

### The database gives space back, and the mode has to survive the pool
Metrics land every few seconds and are pruned in bulk, so deletes free pages that a
default SQLite file never returns. The connection options set
`auto_vacuum = INCREMENTAL` — **sqlx's own default is `None`, which would push the file
back to "never shrink" on the next VACUUM** — and the one-off rebuild that applies the mode
to an existing file runs at startup on a single acquired connection, because the pragma and
the `VACUUM` must be the same connection to take effect. `db::reclaim_free_pages` then runs
`PRAGMA incremental_vacuum` after each retention prune. Reading `PRAGMA auto_vacuum` from a
different pooled connection reports whatever the mode was when *that* connection opened, so
the check reads it back on the connection that did the work.

### A startup self-check answers the first round of questions
`diag::health` reports schema version and migration count, `quick_check` plus free pages and
the auto-vacuum mode, whether every managed runtime is present and still answers `-version`,
and whether each instance folder is reachable. It runs at launch into the log, shows in the
About dialog, and travels in the problem report as `health.txt`. A missing instance folder is
a warning, not a failure — it is recoverable.

### A corrupt database looks like the app forgetting things
A damaged index makes `COUNT(*)` disagree with a table scan and makes `ON CONFLICT` update
the wrong row — which reads as instances vanishing and as runtimes carrying another
runtime's version. `db::integrity_problems` runs `PRAGMA quick_check` at startup; problems
are logged at error level, go into the problem report, and the one disposable table
(`java_runtimes`) is rebuilt from scratch.

### The app can provide the Java itself
Managed runtimes are Temurin JDKs this app downloads from the Adoptium API, one per feature
version under `<data>/runtimes/temurin-<major>/` and **shared by every instance that needs
it** — never a copy per instance. Only the `package` of a release is used, never the
`installer`: an `.msi` would install into the system, and a managed runtime has to vanish
with the app. The archive goes through the Phase 2 download engine (resumable, cancellable,
SHA-256 verified, cached), is unpacked into a sibling staging folder, and only then renamed
into place, so a folder under `runtimes/` never holds a half-extracted JDK.

Selection order is the user's choices before the app's own: pin, then a managed runtime,
then a system JDK (`java::select_for`). Nothing suitable is `None`, which is what the UI
turns into an offer naming the version and the download size — asked while an instance is
being created or imported, not at the first failed start. A managed runtime is removed only
when no instance depends on it, and the refusal names them.

`use_system_java_only` means what it says on the switch — "use only the Java installed on
this computer" — so it stops downloads **and** stops the already-downloaded runtimes being
chosen. That takes two exclusions, not one: `managed::for_version` returns `None`, and the
system route drops paths under `<data>/runtimes/` as well, because `install` also registers
the runtime in the detected list so the pin dropdown can offer it. The filtering happens
before the pick (`java::best_of`), never after it: the managed Java 8 is the *lowest* major
that satisfies a 1.16 server, and rejecting the winner afterwards reported "nothing
suitable" while a usable system Java 17 sat behind it in the list.

### How strict the Java version is depends on the server type
`java::fit_for` decides. Vanilla, Paper and Purpur take **a floor**: any usable JDK at or
above the requirement, lowest first, because they are plain Java programs and a 1.16 server
on Java 17 behaves. Fabric, Forge and NeoForge take **the exact major** their Minecraft
release was built against: Mixin rewrites bytecode as it loads, and a class file format it
does not know about fails somewhere inside a third-party mod rather than saying what is
wrong. So a 1.16.5 Forge server gets Java 8 or an offer to download it — never a silent
Java 17.

A pin still wins under either rule, because it is the user saying they know better, but it
is said out loud: the create dialog shows a warning, `java_status` explains it, and preflight
writes a line into the console before the server starts. Too *old* stays a refusal whoever
chose it — a pin is not permission to run a server on a JVM that cannot load its class files.

`JavaFit::accepts` is the whole rule, and `best_of` applies it to the **list** before picking,
never to the winner afterwards: Java 8 sorts first, so a rule applied after the pick reports
"nothing suitable" for a Java 17 requirement on a machine that has both. Refusing to
substitute has its own error (`java_wrong_major`), because "Java 17 is installed" is not the
same problem as "your Java is too old" and the fix is different.

The reasoning is a sentence built in `java_plan_for`, not a verdict: "1.16.5 Forge is tested
on Java 8; this computer has Java 17…", with the download offer beneath it.

### The required Java version is a floor, and the chosen binary is asked directly
`java::required_for(recorded, mc_version)` takes the higher of the number recorded at
install time and the version table's answer. Mojang's metadata may raise the requirement —
it knows about a new one before the table does — but it may never lower it: a row saying
Java 8 for a 26.2 server let a Java 17 runtime be chosen, and the server died with
`UnsupportedClassVersionError` seconds later. (The wrong row came from computing the
requirement off the artifact's *build* string, a Fabric loader version, rather than the
Minecraft version.)

Preflight then asks the resolved binary what it is (`java::probe_major`) instead of trusting
the cached row, and refuses when it is too old — including a pinned one. A pin is a
preference, not permission to run a server on a JVM that cannot load it.

### Servers never open windows
Every jar launch carries `-Djava.awt.headless=true` unless the user set the property
themselves. Without it Fabric's launcher reports its errors in a Swing dialog, which from a
manager means a stray window holding the message this app should be showing.

### Class file versions are translated before anyone sees them
`UnsupportedClassVersionError` talks in class file numbers ("69.0 … up to 61.0"), which
nobody thinks in. `logparse::parse_class_version_error` converts both (feature version =
class version − 44) into "this server needs Java 25 … ran on Java 17".

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
and `-Xmx8192M` and `-Xmx8192M
` look identical in a console, so this is the first thing
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

### Only one copy of the app runs
A second launch opens the same SQLite file and the same instance folders, and two supervisors
reconciling the same pids is how rows and consoles get lost. `tauri-plugin-single-instance`
is registered **first**, before anything touches the database: the second launch hands its
arguments to the running copy, which unminimizes, shows and focuses its window — in that
order, because an unshown window cannot take focus — and then exits.

### A version is picked from a table, with dates
Two hundred versions in a dropdown is a scroll bar and a guess. `VersionEntry` carries
`release_time` and `kind` from Mojang's manifest, the picker is a table (version, date, type)
with Releases / Snapshots / Pre-releases checkboxes, and the server type is chosen first
because Paper's version list is not Mojang's. The manifest calls every non-release a
`snapshot`, so `mcversion::classify_kind` splits the pre-releases and release candidates out
by the id's own suffix — they are what people go looking for by name. `old_alpha`/`old_beta`
are dropped entirely: Mojang published no server jar before 1.2.5.

### Addresses are labelled by block, never by adapter name
Adapter names are localised, renamed and driver-specific; the address block is the stable
part. 25.x is Hamachi, 26.x is Radmin, 100.64–100.127 is Tailscale (shared with carrier NAT,
hence "usually"), the RFC1918 blocks are the LAN. `net::classify` is pure with a fixture per
range, including the addresses one step outside each one.

### The port check answers two different questions
From this machine we can only see whether something is listening (`local_port_state`, a
connect rather than a bind — a bind answers the opposite question). Whether the internet can
reach it is asked of an outside service, and a failed check reports `reachable: None` with a
sentence saying it proves nothing. "Closed" when the truth is "the status API was down" sends
somebody off to rewrite router settings that were fine.

### UPnP is offered, never assumed
A mapping is a change to somebody's router, so it happens on a click and nowhere else, with
a 12-hour lease rather than a permanent entry that outlives the app. Every failure path ends
in the manual steps, which name the actual gateway address rather than saying "log into your
router". A router whose own external address is private is carrier-grade NAT, and the tab
says so instead of letting someone re-do their port forwarding all evening.

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

### A first boot has one expected error
A server with no `server.properties` logs `Failed to load properties from file` at ERROR
with a full `java.nio.file.NoSuchFileException` trace, then starts normally and writes the
file itself. That block is the expected sequence on a first boot and reads as a serious
failure, so `ConsoleBuffer::expect_missing_properties` is armed for exactly that launch —
`last_started_at` is null **and** no properties file is on disk, both read before the row
is updated — and it turns the header into one info-level sentence with the frames under it
at debug. Nothing is dropped: `raw` still holds what the server printed, so search, copy
and the rotated files are unchanged. The grace is spent on the first complaint and cleared
when the next launch arms the buffer, because on any later boot a missing properties file
is a real problem and the trace is the useful part of it.

### Log formats differ per family
Vanilla, Paper, Forge (log4j plus a logger bracket) and Fabric (a parenthesised logger) all
print differently, and `logparse` handles each; unparsable lines are still shown verbatim.
Test any parser change against the recorded samples in `tests/fixtures/log_*.txt`.

A line that declares its own level keeps it, whatever stream it arrived on: the JVM writes
`WARNING: ...` to stderr and Minecraft 26 prints eight of them about `sun.misc.Unsafe` on
every start, so classifying by stream alone painted a normal boot red. `declared_level`
reads a level word followed by a colon at the start of the line, and the stream is only the
fallback for a line that says nothing about itself (a stack trace, the `Failed to add PDH
Counter` noise). A recognised bracketed format always wins over both.

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

### The browser asks one source at a time, and says which
`AnySource` is the enum that holds whichever implementation was asked for — the trait uses
`impl Future`, so it cannot be a trait object, and an enum keeps every call site written
once. A `PlannedMod` records the source it came from and `check_updates` runs per source,
because a file reference from one means nothing to the other.

CurseForge requires every application to use its own key and forbids shipping one, so an
absent key is a *state*: the source is listed as available-but-unconfigured with the reason
and a link, never hidden. An author may also forbid third-party downloads
(`allowModDistribution: false`), which produces files with no URL — the card says so and
links to the page rather than failing at install time.

Content types are offered by server type: Paper browses plugins, the mod loaders browse mods
and packs, vanilla browses data packs, and the client-only kinds (resource packs, shaders)
are shown for everyone but marked and never installable. Icons are cached under
`<data>/cache/icons/` by a hash of their URL and read through the asset protocol, whose
scope is that folder alone.

### A version is chosen, not assumed
A card's Install is a shortcut for "the newest file that fits", and it says so. The detail
panel is where a particular file is picked: every version the source published, newest by
its own publish date, with channel, Minecraft versions, date and size — and a toggle that
shows the ones that do not fit, each labelled with the reason ("Forge only", "for 1.20.1")
rather than hidden. Dependencies are listed per version, because they differ between them.

An installed mod is marked in that list and can be switched to any other version, downgrade
included; `mods.version_id` is what marks it. Installing another version of a project
removes the previous file and its row (`replace_other_versions`), including a
`.jar.disabled` one — two copies of the same mod in `mods/` is a crash on boot.

Each source downloads only from its own CDN (`download_host_allowed`): a version resolved
from one must not be able to point the downloader at the other's, or anywhere else.

### Dependency resolution is confirmed before anything downloads
Required dependencies are followed recursively into a plan the user confirms; optional ones
are listed and never installed on their own; two versions of one project is a conflict that
is refused by name. Everything installed is recorded in `mods`/`mod_dependencies` so an
uninstall can say what depended on it.

### A pack is checked for a server build before it is offered
Most modpacks are built for a client, and finding that out half way through an install is
what this check exists to avoid. Two levels of certainty, in order: the source's own answer
where it has one (Modrinth publishes `server_side`, and a pack search sends that facet), and
otherwise the pack index itself — a loader this app can run as a server, plus at least one
file not marked `unsupported` for the server. `packs::examine` downloads the pack to read
that index, except when the source has already said no. A pack that cannot run says which
of the two reasons applies, and the install button stays disabled.

Installing a pack **creates** an instance rather than filling one: server type from the
pack's loader, Minecraft version from its index, RAM from what the pack's own text asks for
(`published_ram_mb`) or a suggestion sized to the pack, and then the Phase 5 importer
applies the files with the `env` filtering and the staging it already has.

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
  Networking, Settings. App-wide options live in their own Settings screen (gear in the
  titlebar), never in an instance's tab — an instance tab is unreachable until an instance
  exists, which is exactly when the CurseForge key and the Java list are wanted.
- Everything is keyboard-navigable. The console has an autoscroll toggle, search, and copy.
- **Scrolling containers are built for a small window.** Every flex column between the
  window and a scroll area carries `min-h-0`, or the child cannot shrink, grows past the
  window, and `body { overflow: hidden }` makes the overflow unreachable — `TabsContent`
  sets it for every tab so none can forget. Dialogs cap at `max-h-[90vh]` with
  `overflow-y-auto`, and `DialogFooter` is sticky so its buttons stay reachable while the
  content above scrolls. The open animation may only animate `opacity` and `scale`:
  animating `transform` stacks with Tailwind's `translate` centring and pushes the dialog
  off screen.
- **A dialog's sticky footer must not shorten the scroll area.** `DialogContent` carries
  `px-6 pt-6 pb-0` and the footer supplies `pb-6`: a negative bottom margin on a sticky
  footer removes exactly that much from the scroll height, and the last line of content ends
  up underneath it with nowhere left to scroll (measured: 8px of "1 file to download · 2.4 MB"
  hidden at 1000x700).
- **Copying goes through `lib/clipboard.ts`.** `navigator.clipboard` needs a secure context
  and the webview's custom scheme is not one everywhere, so the plugin does it and a failure
  is reported rather than swallowed — the same shape as the dead-link bug.
- **External addresses go through `lib/external.ts`.** One helper, awaited, http and https
  only, and it reports a failure instead of swallowing it — `void openUrl(...)` turned a
  missing opener scope into a dead button everywhere at once. The opener capability needs
  `opener:default` *and* a URL scope; `opener:allow-open-url` on its own grants the command
  with nothing it is allowed to open.
- `src/components/ui/layout.test.ts` checks these rules across every `.tsx` file: a capped
  height with no overflow, a `flex-1` scroll area that cannot shrink, a dialog footer with a
  negative margin, and any address opened by a route other than the helper.
- **The console's autoscroll releases on a user scroll and only on a user scroll.** The jump
  to the bottom sets a flag that the next scroll event clears, because the browser delivers
  that event a frame later and it is otherwise indistinguishable from the user returning —
  which is what made scrolling up impossible on a server printing continuously. The flag is
  only armed when the view really moves, and is cleared on the next frame regardless, so it
  can never swallow a real scroll.
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
