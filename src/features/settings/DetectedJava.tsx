import { open } from "@tauri-apps/plugin-dialog";
import { FolderOpen, RefreshCw } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/misc";
import { isUsable, scanAgeLabel, unsuitableReason } from "@/features/setup/javaLabels";
import { useAddJava, useJavaRuntimes, useRescanJava } from "@/features/setup/queries";

/**
 * Every Java this machine has, as detection sees it.
 *
 * The list is app-wide — instances only pin one of these — so it belongs here
 * rather than inside one server's tab, where it was invisible until an
 * instance existed.
 */
export function DetectedJava({ lastScanAt, stale }: { lastScanAt: string | null; stale: boolean }) {
  const runtimes = useJavaRuntimes();
  const rescan = useRescanJava();
  const addJava = useAddJava();

  async function browse() {
    const picked = await open({
      directory: true,
      title: "Choose a JDK folder (or its bin folder)",
    });
    if (typeof picked === "string") addJava.mutate(picked);
  }

  const rows = runtimes.data ?? [];

  return (
    <section className="grid gap-3">
      <header className="flex flex-wrap items-center justify-between gap-2">
        <div>
          <h3 className="text-sm font-semibold">Java found on this computer</h3>
          {/* A JDK installed since the last scan is simply absent, and a list
              that looks complete gives no hint of that. */}
          <p className={stale ? "text-xs text-[var(--status-starting)]" : "text-xs text-muted-foreground"}>
            {scanAgeLabel(lastScanAt)}
            {rows.length > 0 ? ` · ${rows.length} found` : ""}
          </p>
        </div>
        <div className="flex items-center gap-2">
          <Button variant="outline" size="sm" disabled={rescan.isPending} onClick={() => rescan.mutate()}>
            <RefreshCw /> {rescan.isPending ? "Scanning…" : "Rescan"}
          </Button>
          <Button variant="outline" size="sm" onClick={() => void browse()}>
            <FolderOpen /> Browse for JDK…
          </Button>
        </div>
      </header>

      {rows.length === 0 ? (
        <p className="rounded-md border border-border px-3 py-2 text-xs text-muted-foreground">
          None found. Rescan after installing one, browse to a folder detection missed, or let
          the app download one below.
        </p>
      ) : (
        <ul className="grid gap-1.5">
          {rows.map((runtime) => {
            const reason = unsuitableReason(runtime);
            return (
              <li
                key={runtime.id}
                className={
                  isUsable(runtime)
                    ? "flex flex-wrap items-center gap-2 rounded-md border border-border px-3 py-2 text-xs"
                    : "flex flex-wrap items-center gap-2 rounded-md border border-border px-3 py-2 text-xs opacity-60"
                }
              >
                <Badge>Java {runtime.major}</Badge>
                {runtime.vendor ? <span>{runtime.vendor}</span> : null}
                <span className="font-mono text-muted-foreground">{runtime.path}</span>
                {reason ? <span className="ml-auto text-muted-foreground">{reason}</span> : null}
              </li>
            );
          })}
        </ul>
      )}
    </section>
  );
}
