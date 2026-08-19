import { useQuery } from "@tanstack/react-query";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { FolderOpen } from "lucide-react";

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
