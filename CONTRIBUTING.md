# Contributing

## Building

You need [Rust](https://rustup.rs), [Node 24+](https://nodejs.org) and
[pnpm](https://pnpm.io) 11. The Rust version is pinned in `rust-toolchain.toml`
and rustup installs it for you on the first `cargo` command — do not reach for
`stable`, because `-D warnings` turns every lint added to a newer compiler into
a build break, and CI would then fail on a commit that was green on your
machine. On Linux you also need the WebKitGTK development
packages Tauri builds against:

```bash
sudo apt-get install libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev patchelf
```

Then:

```bash
pnpm install
pnpm tauri dev
```

`pnpm tauri build` produces installers for the platform you are on: `.msi` and
`.exe` on Windows, `.deb` and `.AppImage` on Linux.

## Tests

The gate every change has to pass:

```bash
cargo test --manifest-path src-tauri/Cargo.toml
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
pnpm typecheck
pnpm test
```

Rust tests do not use the network. Providers are generic over a `Fetch` trait
and the tests drive them with `FixtureFetch`, which maps a URL to a recorded
payload and fails on any URL a test did not record.

## Live tests

Anything that talks to a real service is `#[ignore]`, so `cargo test` never
reaches it. They exist because a fixture proves this app agrees with itself and
nothing more — the interesting failures have all been the difference between
what a fixture said and what the real thing does.

```bash
# Everything live: Modrinth, CurseForge, Adoptium, Mojang, the installers.
cargo test --manifest-path src-tauri/Cargo.toml --test network_smoke -- --ignored --nocapture

# The two heaviest, worth knowing about before you start them:
#   downloads a JDK and boots a real 1.16.5 server
cargo test --manifest-path src-tauri/Cargo.toml --test managed_runtime_walk -- --ignored --nocapture
#   installs the map, boots a real server on Fabric and on Paper, and checks
#   the map answers on the port this app chose
cargo test --manifest-path src-tauri/Cargo.toml --test network_smoke the_map_opens_on_the_port -- --ignored --nocapture
```

They download server jars, mods and JDKs into temporary folders, take minutes,
and need a working internet connection. Run them by hand after touching
providers, the downloader, the installer or the map.

## Recorded fixtures

The fixtures in `src-tauri/tests/fixtures/` pin real API payloads, and the
upstream services eventually retire the builds they mention. Re-record them
with:

```bash
cargo xtask refresh-fixtures
```

Then read the diff and re-run the tests. A test that breaks only because Paper
shipped a new build is a badly written test: assert on shapes and relationships
(newest-first ordering, checksum length, URL construction), never on a specific
build number.

## Conventions

`CLAUDE.md` is the long version: what lives where, and — more usefully — the
domain rules that exist because they went wrong once. It is worth reading the
section headings before changing anything in `java/`, `process/`, `map/` or
`config/`.

Commits use conventional-commit subjects (`feat:`, `fix:`, `chore:`) and a body
that says what changed and why.
