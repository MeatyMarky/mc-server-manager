# Troubleshooting

Each section starts with what you see on screen. If the app showed you an error, it also
showed a **Details** expander — the text in there is what to quote in a bug report.

---

## "Java 21 or newer is needed, and no Java was found on this computer."

A Minecraft server is a Java program and there is no Java installed, or it is somewhere the
app does not look.

1. Install a **JDK**: [Adoptium Temurin](https://adoptium.net/temurin/releases/) on any
   platform, `sudo apt install openjdk-21-jdk` on Debian and Ubuntu.
2. In the app: **Settings → Java → Rescan**.
3. Still not found? Use **Browse for a JDK** and point at the folder you installed it into
   (the one containing `bin/java`). The app never guesses from folder names — it runs
   `java -version` and believes the answer.

## "Minecraft 1.21.4 needs Java 21, but the newest Java on this computer is Java 17."

You have Java, but it is older than this Minecraft version accepts. Installing a newer one
does not remove the old one, and the app picks per server, so nothing else breaks.

Install Java 21 (or 25 for the 26.x calendar versions), then **Settings → Java → Rescan**.
If a specific server needs to keep using the old one, pin it in that server's **Settings**
tab.

## "Port 25565 is already being used by …"

Two servers cannot share a port.

- **Used by another of your servers:** stop that one, or change this one's port in
  **Config → server-port**.
- **Used by another program:** something else on the machine holds it — often a Minecraft
  server left running from an earlier session.
  - Windows: `netstat -ano | findstr :25565`, then look up the PID in Task Manager.
  - Linux: `ss -ltnp | grep 25565`.
- After a crash or a forced shutdown, the app may still show the old server as
  **Unmanaged** — running, but not started by this app. Use **Force stop** on it.

## "The drive holding … has no space left."

Backups and server downloads both need room, and the app refuses to start a backup it
cannot finish (it checks for the estimated size plus 20%).

Free some space, or put the server on another drive. Old backups are a good place to
start — **Backups** tab, or set a retention limit on the schedule so it prunes itself.

## "The app could not reach the internet."

The app talks to Mojang, PaperMC, Purpur, Fabric, Forge, NeoForge and Modrinth. If none of
them answer:

- Check the connection generally.
- A VPN, a corporate proxy or a firewall blocking the app looks identical to being offline.
- One provider being down looks different: you will get an error naming that one service,
  and the others will still work.

## "api.modrinth.com is asking the app to slow down."

Modrinth publishes a request budget and the app stays inside it, but a burst of searches
can still hit the limit. Wait the number of seconds the message names and try again.
Nothing was lost, and no installed mod is affected.

## "The folder for … is not where the app left it."

The server's folder was moved, renamed, or is on a drive that is not mounted. This is not
data loss and the app does not delete anything.

- If it moved: click **Locate folder…** on the server and point at its new home.
- If the drive is disconnected: reconnect it; the server goes back to normal by itself.

## "… is missing files it needs to run." / the server exits immediately

Usually a half-finished install, or files removed by hand.

1. Open the **Console** tab and read the last lines — the server usually says what it
   wants.
2. **Settings → Install** re-downloads the server files. Worlds, configuration and mods are
   left alone.
3. If a Forge or NeoForge install failed, the app kept the installer's full log next to the
   instance under `.msm/installer-*.log`, and the error message names the path.

## The server starts and then stops with "You need to agree to the EULA"

Nothing accepted the EULA, and this app deliberately never does it for you. Open the
server's **Settings** tab, read the linked agreement, and tick the box. That writes
`eula=true` to that server's `eula.txt`, and only then.

## A backup finished but the world looks wrong / saving seems disabled

While a running server is backed up, the app turns saving off, waits for the server to
confirm it has flushed, archives, and turns saving back on. If the app is killed in the
middle of that:

- If the server also stopped, saving is on again the moment it next starts — the setting
  only lives in the running process.
- If the server kept running without the app, start it from the app once; saving is
  re-enabled as soon as it reports ready. The app log says so.

The app refuses to back up a server that is running but was **not** started from here,
because it cannot pause saving on it and would archive a half-written world.

## Windows says the app is unrecognised

The installers are not code-signed. SmartScreen shows "Windows protected your PC" — choose
**More info → Run anyway**.

Signing would mean buying a code-signing certificate (an OV certificate is a few hundred
currency units a year; an EV one, which clears SmartScreen immediately, costs more and
usually ships on a hardware token or an HSM), then signing both the `.exe` and the `.msi`
in CI with `signtool`, with the certificate held in a secret store. Tauri supports this
through `bundle.windows.certificateThumbprint` and `signCommand`. It is a paperwork and
money problem, not a technical one.

## Linux: the AppImage will not start

```bash
chmod +x Minecraft*.AppImage
./Minecraft*.AppImage
```

If it complains about `libwebkit2gtk`, install the system webview:

```bash
sudo apt install libwebkit2gtk-4.1-0     # Debian, Ubuntu
sudo dnf install webkit2gtk4.1           # Fedora
```

## Closing the window did not quit the app

That is deliberate: the window closes to the tray and your servers keep running. Quit from
the tray icon, and the app warns you if servers are still alive.

---

## Reporting a bug

Use the **?** button in the top-right corner. It collects:

- the app's own log (last N lines, you choose N),
- the Java runtimes it detected,
- version, commit and database schema version,
- the selected server's settings and console tail.

Every part is shown in full **before** anything is written, and the file is saved where you
choose. The app never uploads it. Paths in it contain your user name, and the console can
contain player names and chat — worth a read before attaching it to a public issue.
