import { open } from "@tauri-apps/plugin-dialog";
import { useQuery } from "@tanstack/react-query";
import {
  Activity,
  FolderOpen,
  HelpCircle,
  Info,
  Monitor,
  Moon,
  RotateCcw,
  Sun,
} from "lucide-react";
import { useEffect, useState } from "react";

import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { CurseForgeKey } from "@/features/setup/CurseForgeKey";
import { ManagedRuntimes } from "@/features/setup/ManagedRuntimes";
import { ipc } from "@/lib/ipc";
import { applyTheme, useUiStore, type Theme } from "@/stores/ui";
import { useAppInfo } from "@/features/instances/queries";
import { DetectedJava } from "./DetectedJava";
import { useSetSetting, useSettings } from "./queries";

/**
 * Everything that belongs to the app rather than to one server.
 *
 * These panels existed before this screen did, and lived inside an instance's
 * Settings tab — which meant the CurseForge key and the managed Java list were
 * unreachable until a server existed, and read as per-server settings once one
 * did.
 */
export function AppSettings({
  onAbout,
  onReportProblem,
}: {
  onAbout: () => void;
  onReportProblem: () => void;
}) {
  const settings = useSettings();
  const info = useAppInfo();
  const scan = useQuery({ queryKey: ["java-scan-info"], queryFn: () => ipc.javaScanInfo() });

  return (
    <div className="min-h-0 flex-1 overflow-y-auto px-6 pb-10">
      <div className="mx-auto grid max-w-3xl gap-8 py-2">
        <header>
          <h2 className="text-lg font-semibold">Settings</h2>
          <p className="text-sm text-muted-foreground">
            These apply to the whole app. Per-server options live in that server's Settings tab.
          </p>
        </header>

        <Appearance />

        <section className="grid gap-3">
          <h3 className="text-sm font-semibold">Folders</h3>
          <DefaultRoot
            suggested={info.data?.defaultInstanceRoot ?? ""}
            stored={settings.data?.default_instance_root}
          />
          <div className="grid gap-1">
            <Label>App data</Label>
            <p className="rounded-md border border-border px-3 py-2 font-mono text-xs text-muted-foreground">
              {info.data?.dataDir ?? "…"}
            </p>
            <p className="text-xs text-muted-foreground">
              Holds the database, downloaded Java, backups and cached files.
            </p>
          </div>
        </section>

        <div className="grid gap-6 border-t border-border pt-6">
          <DetectedJava
            lastScanAt={scan.data?.lastScanAt ?? null}
            stale={scan.data?.scanIsStale ?? false}
          />
          <ManagedRuntimes />
        </div>

        <div className="border-t border-border pt-6">
          <CurseForgeKey />
        </div>

        <div className="border-t border-border pt-6">
          <MetricsInterval stored={settings.data?.metrics_interval_seconds} />
        </div>

        <section className="grid gap-3 border-t border-border pt-6">
          <h3 className="text-sm font-semibold">This app</h3>
          <div className="flex flex-wrap gap-2">
            <Button variant="outline" size="sm" onClick={onAbout}>
              <Info /> About and self-check
            </Button>
            <Button variant="outline" size="sm" onClick={onReportProblem}>
              <HelpCircle /> Report a problem
            </Button>
          </div>
        </section>
      </div>
    </div>
  );
}

/** Theme, as a choice rather than a toggle that only shows what comes next. */
function Appearance() {
  const { theme, setTheme } = useUiStore();
  const save = useSetSetting();

  function choose(next: Theme) {
    setTheme(next);
    applyTheme(next);
    save.mutate({ key: "theme", value: next });
  }

  return (
    <section className="grid gap-3">
      <h3 className="flex items-center gap-2 text-sm font-semibold">
        <Monitor className="size-4" aria-hidden />
        Appearance
      </h3>
      <div className="flex gap-2" role="group" aria-label="Theme">
        <Button
          variant={theme === "dark" ? "default" : "outline"}
          size="sm"
          aria-pressed={theme === "dark"}
          onClick={() => choose("dark")}
        >
          <Moon /> Dark
        </Button>
        <Button
          variant={theme === "light" ? "default" : "outline"}
          size="sm"
          aria-pressed={theme === "light"}
          onClick={() => choose("light")}
        >
          <Sun /> Light
        </Button>
      </div>
    </section>
  );
}

/**
 * Where new instance folders are suggested.
 *
 * A suggestion only: an instance may live anywhere, and changing this never
 * moves one that already exists.
 */
function DefaultRoot({ suggested, stored }: { suggested: string; stored: string | undefined }) {
  const save = useSetSetting();
  const [draft, setDraft] = useState(suggested);

  useEffect(() => setDraft(suggested), [suggested]);

  async function browse() {
    const picked = await open({ directory: true, title: "Choose where new servers go" });
    if (typeof picked === "string") {
      setDraft(picked);
      save.mutate({ key: "default_instance_root", value: picked });
    }
  }

  return (
    <div className="grid gap-1.5">
      <Label htmlFor="default-root">Default folder for new servers</Label>
      <div className="flex flex-wrap gap-2">
        <Input
          id="default-root"
          className="min-w-72 flex-1 font-mono text-xs"
          value={draft}
          onChange={(event) => setDraft(event.target.value)}
        />
        <Button variant="outline" size="sm" onClick={() => void browse()}>
          <FolderOpen /> Browse…
        </Button>
        <Button
          size="sm"
          disabled={save.isPending || draft.trim() === "" || draft === (stored ?? suggested)}
          onClick={() => save.mutate({ key: "default_instance_root", value: draft.trim() })}
        >
          Save
        </Button>
        {stored ? (
          <Button
            variant="ghost"
            size="sm"
            disabled={save.isPending}
            onClick={() => save.mutate({ key: "default_instance_root", value: "" })}
            title="Go back to the folder inside the app's data directory"
          >
            <RotateCcw /> Reset
          </Button>
        ) : null}
      </div>
      <p className="text-xs text-muted-foreground">
        Pre-fills the create dialog. Servers can live anywhere, and this never moves one that
        already exists.
      </p>
    </div>
  );
}

/** How often running servers are sampled for the charts. */
function MetricsInterval({ stored }: { stored: string | undefined }) {
  const save = useSetSetting();
  const current = stored ?? "5";
  const [draft, setDraft] = useState(current);

  useEffect(() => setDraft(current), [current]);

  return (
    <section className="grid gap-2">
      <h3 className="flex items-center gap-2 text-sm font-semibold">
        <Activity className="size-4" aria-hidden />
        Performance charts
      </h3>
      <div className="flex flex-wrap items-end gap-2">
        <div className="grid gap-1.5">
          <Label htmlFor="metrics-interval">Sample every (seconds)</Label>
          <Input
            id="metrics-interval"
            type="number"
            min={1}
            max={300}
            className="w-32"
            value={draft}
            onChange={(event) => setDraft(event.target.value)}
          />
        </div>
        <Button
          size="sm"
          disabled={save.isPending || draft === current}
          onClick={() =>
            save.mutate({
              key: "metrics_interval_seconds",
              // The backend clamps to 1–300 too; this keeps the field from
              // storing a number it would silently ignore.
              value: String(Math.min(300, Math.max(1, Number(draft) || 5))),
            })
          }
        >
          Save
        </Button>
      </div>
      <p className="text-xs text-muted-foreground">
        One sample per running server per tick. Full resolution is kept for 24 hours, then
        thinned to a row a minute, and deleted after 30 days.
      </p>
    </section>
  );
}
