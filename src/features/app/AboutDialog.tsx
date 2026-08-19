import { useQuery } from "@tanstack/react-query";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { AlertTriangle, CheckCircle2, FolderOpen, XCircle } from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { ipc } from "@/lib/ipc";
import type { HealthStatus } from "@/lib/types";

/**
 * Version, commit, and where the app keeps things.
 *
 * The paths are here because they are the first thing anyone is asked for when
 * something goes wrong, and hunting for an app data folder is miserable.
 */
export function AboutDialog({
  open,
  onOpenChange,
  onReportProblem,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onReportProblem: () => void;
}) {
  const info = useQuery({
    queryKey: ["build-info"],
    queryFn: () => ipc.buildInfo(),
    enabled: open,
  });

  // The self-check: one place to see whether this install is healthy.
  const health = useQuery({
    queryKey: ["health"],
    queryFn: () => ipc.healthCheck(),
    enabled: open,
  });

  const build = info.data;

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Minecraft Server Manager</DialogTitle>
          <DialogDescription>
            {build
              ? `Version ${build.version} · ${build.platform} ${build.arch}`
              : "Reading build information…"}
          </DialogDescription>
        </DialogHeader>

        {build ? (
          <dl className="grid gap-2 text-sm">
            <Row label="Version" value={build.version} />
            <Row label="Commit" value={build.gitSha} mono />
            <Row
              label="Database"
              value={build.dbPath}
              mono
              onReveal={() => void revealItemInDir(build.dbPath)}
            />
            <Row
              label="Logs"
              value={build.logDir}
              mono
              onReveal={() => void revealItemInDir(build.logDir)}
            />
            <Row
              label="Instance root"
              value={build.instanceRoot}
              mono
              onReveal={() => void revealItemInDir(build.instanceRoot)}
            />
            <Row
              label="Schema version"
              value={build.schemaVersion === null ? "unknown" : String(build.schemaVersion)}
            />
          </dl>
        ) : null}

        {health.data ? (
          <section className="grid gap-1.5" aria-label="Self-check">
            <h3 className="flex items-center gap-2 text-sm font-medium">
              <StatusIcon status={health.data.status} />
              {health.data.status === "ok"
                ? "Everything checks out"
                : health.data.status === "warn"
                  ? "Working, with something worth knowing"
                  : "Something is wrong"}
            </h3>
            <ul className="grid gap-1 text-xs">
              {health.data.checks.map((check) => (
                <li key={check.name} className="flex items-start gap-2">
                  <StatusIcon status={check.status} />
                  <span className="min-w-0">
                    <span className="font-medium">{check.name}: </span>
                    <span className="text-muted-foreground">{check.detail}</span>
                  </span>
                </li>
              ))}
            </ul>
          </section>
        ) : null}

        <DialogFooter>
          <Button variant="outline" onClick={onReportProblem}>
            Report a problem
          </Button>
          <Button onClick={() => onOpenChange(false)}>Close</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function Row({
  label,
  value,
  mono = false,
  onReveal,
}: {
  label: string;
  value: string;
  mono?: boolean;
  onReveal?: () => void;
}) {
  return (
    <div className="flex items-start justify-between gap-3 border-b border-border/60 pb-2 last:border-none">
      <dt className="shrink-0 text-muted-foreground">{label}</dt>
      <dd className="flex min-w-0 items-center gap-2">
        <span className={mono ? "truncate font-mono text-xs" : "truncate"} title={value}>
          {value}
        </span>
        {onReveal ? (
          <Button
            size="icon"
            variant="ghost"
            aria-label={`Open the ${label.toLowerCase()} location`}
            onClick={onReveal}
          >
            <FolderOpen />
          </Button>
        ) : null}
      </dd>
    </div>
  );
}

/**
 * The state of one check. Colour is doubled by the icon shape and the label
 * that follows it, so it never carries the meaning on its own.
 */
function StatusIcon({ status }: { status: HealthStatus }) {
  if (status === "ok") {
    return (
      <>
        <CheckCircle2 className="mt-0.5 size-3.5 shrink-0 text-emerald-500" aria-hidden />
        <span className="sr-only">Passed: </span>
      </>
    );
  }
  if (status === "warn") {
    return (
      <>
        <AlertTriangle
          className="mt-0.5 size-3.5 shrink-0 text-[var(--status-starting)]"
          aria-hidden
        />
        <span className="sr-only">Warning: </span>
      </>
    );
  }
  return (
    <>
      <XCircle className="mt-0.5 size-3.5 shrink-0 text-destructive" aria-hidden />
      <span className="sr-only">Failed: </span>
    </>
  );
}
