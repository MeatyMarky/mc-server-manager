# Minecraft Server Manager

A desktop app for running Minecraft servers on your own computer. It downloads the server
files, starts and stops them, shows the console, installs mods and plugins, and keeps
backups — for as many servers as you like, side by side.

Windows and Linux are both first-class. Nothing is uploaded anywhere: every server, world
and backup stays on your machine.

---

## For people who just want to run a server

### 1. Install Java

A Minecraft server is a Java program, so you need Java on the computer. If you are not sure
whether you have it, install it anyway — having two copies causes no harm.

- **Any platform:** [Adoptium Temurin](https://adoptium.net/temurin/releases/) — pick the
  **JDK**, the newest **LTS** version (Java 21 or newer).
- **Windows:** the `.msi` installer from Adoptium is the simplest route.
- **Debian / Ubuntu:** `sudo apt install openjdk-21-jdk`
- **Fedora:** `sudo dnf install java-21-openjdk`

Which Java a server needs depends on its Minecraft version, and the app works that out on
its own. Java 21 covers current versions; older servers may want Java 17 or Java 8, and the
app will say so if it needs one you do not have.

### 2. Install the app

Download the installer for your system from the
[Releases page](https://github.com/your-org/mc-server-manager/releases):

| System | File | Notes |
| --- | --- | --- |
| Windows | `.msi` or `.exe` (NSIS) | Either works. The `.msi` suits managed machines. |
| Debian, Ubuntu, Mint | `.deb` | `sudo apt install ./Minecraft*.deb` |
| Other Linux | `.AppImage` | `chmod +x Minecraft*.AppImage`, then run it. |

The builds are **not code-signed**. Windows SmartScreen will warn you the first time:
choose **More info → Run anyway**. See
[docs/troubleshooting.md](docs/troubleshooting.md#windows-says-the-app-is-unrecognised) for
what signing would involve.

### 3. Create your first server

1. Open the app. It looks for Java in the background and tells you what it found.
2. Click **Create a server**.
3. Choose a name, a folder, a Minecraft version, and a type:
   - **Vanilla** — exactly what Mojang ships.
   - **Paper** / **Purpur** — much faster, and they take **plugins**.
   - **Fabric** / **Forge** / **NeoForge** — they take **mods**.
   If you are unsure, pick Paper.
4. Wait for the download to finish.
5. Open the **Settings** tab and **accept the Minecraft EULA**. The app never accepts it
   for you — a server refuses to start until you do.
6. Press **Start**.

The **Console** tab shows the server's output live. Type commands into the box at the
bottom, exactly as you would in a terminal.

### 4. Let people in

- Others on your home network connect to your computer's local IP, port `25565`.
- Over the internet, you have to forward port `25565` on your router to this computer.
  That is a router setting, not an app setting.
- **Players**, **Worlds**, **Config** and **Backups** each have their own tab.

### Where your files live

The **About** dialog (the ⓘ button, top right) shows the exact paths on your machine,
with a button to open each one:

| What | Windows | Linux |
| --- | --- | --- |
| App data, database, logs | `%APPDATA%\dev.msm.manager` | `~/.local/share/dev.msm.manager` |
| Servers | wherever you chose when creating them | same |
| Backups | `<app data>/backups/<server id>/` | same |

### When something goes wrong

Every error in the app is written in plain language, with a **Details** expander holding
the technical text. [docs/troubleshooting.md](docs/troubleshooting.md) covers the common
ones.

If you need to report a bug, use the **?** button in the top-right corner. It builds a zip
with the app log, your Java setup and the server's console — and **shows you every line
before writing it**, so you can check what you are about to share. The app never sends
anything anywhere itself.

---

## For developers

### Stack

Tauri v2 (Rust backend, system webview), React + TypeScript + Vite, Tailwind + shadcn/ui,
tokio, sqlx + SQLite. Package manager: pnpm. **No Electron.**

All process, filesystem and network work lives in Rust; the frontend calls Tauri commands
and subscribes to events, and never polls. `CLAUDE.md` documents the conventions and the
domain rules that must not be "simplified away".

### Build from source

Prerequisites: Rust (stable), Node 24+, pnpm 11+. On Linux also:

```bash
sudo apt install libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev patchelf
```

```bash
pnpm install
pnpm tauri dev
```

Installers for the current platform:

```bash
pnpm tauri build
```

### Tests

```bash
cargo test           # in src-tauri/, no network
pnpm test            # vitest
pnpm typecheck
cargo clippy --all-targets -- -D warnings
```

Provider tests run against recorded fixtures. Re-record them with `cargo xtask
refresh-fixtures` when an upstream API retires a build — stale fixtures are maintenance,
not a break. Live checks are `#[ignore]`d:

```bash
cargo test --test network_smoke -- --ignored --nocapture
```

### Releasing

Bump `version` in `src-tauri/Cargo.toml` — that is the only place it lives, and Tauri reads
it from there — then tag the commit `vX.Y.Z` and push. `.github/workflows/release.yml`
checks the tag against the crate version, builds MSI, NSIS, `.deb` and AppImage, and drafts
a GitHub release with the artefacts attached. The commit SHA is stamped into the binary by
`build.rs` and shown in the About dialog.

---

Not affiliated with Mojang or Microsoft. Minecraft is a trademark of Mojang Synergies AB.
