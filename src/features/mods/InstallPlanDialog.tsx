import { useQuery } from "@tanstack/react-query";
import { CornerDownRight, Download } from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Badge } from "@/components/ui/misc";
import { formatBytes } from "@/lib/format";
import { errorMessage, ipc } from "@/lib/ipc";
import { toastError } from "@/lib/toast";
import type { InstanceView, Project } from "@/lib/types";

/**
 * Shows the resolved dependency tree and asks for confirmation. Nothing is
 * downloaded until the user says yes; optional dependencies are listed here and
 * never installed on their own.
 */
export function InstallPlanDialog({
  instance,
  project,
  versionId,
  onClose,
}: {
  instance: InstanceView;
  project: Project | null;
  /// The file the user picked, or null for "the newest that fits".
  versionId?: string | null;
  onClose: () => void;
}) {
  const plan = useQuery({
    queryKey: ["mod-plan", instance.id, project?.source, project?.id, versionId ?? null],
    // The source the card came from: a CurseForge file id means nothing to
    // Modrinth, and the other way round.
    queryFn: () => ipc.modsPlan(instance.id, project!.source, project!.id, versionId ?? null),
    enabled: project !== null,
    retry: false,
  });

  async function confirm() {
    if (!plan.data) return;
    try {
      await ipc.modsInstall(instance.id, plan.data);
      onClose();
    } catch (error) {
      toastError(error);
    }
  }

  const toInstall = plan.data?.install.filter((entry) => !entry.alreadyInstalled) ?? [];

  return (
    <Dialog open={project !== null} onOpenChange={(open) => (open ? null : onClose())}>
      <DialogContent className="max-w-2xl">
        <DialogHeader>
          <DialogTitle>Install {project?.title}</DialogTitle>
          <DialogDescription>
            Required dependencies are installed with it. Nothing is downloaded until you
            confirm.
          </DialogDescription>
        </DialogHeader>

        {plan.isLoading ? (
          <p className="text-sm text-muted-foreground">Resolving dependencies…</p>
        ) : plan.isError ? (
          <p className="text-sm text-destructive">{errorMessage(plan.error)}</p>
        ) : plan.data ? (
          <div className="grid gap-4">
            <ul className="grid gap-1">
              {plan.data.install.map((entry) => (
                <li
                  key={entry.versionId}
                  className="flex items-center justify-between gap-3 rounded-md border border-border px-3 py-2"
                >
                  <span
                    className="flex min-w-0 items-center gap-2"
                    style={{ paddingLeft: `${Number(entry.depth) * 16}px` }}
                  >
                    {Number(entry.depth) > 0 ? (
                      <CornerDownRight className="size-3.5 shrink-0 text-muted-foreground" />
                    ) : null}
                    <span className="min-w-0">
                      <span className="block truncate text-sm font-medium">
                        {entry.projectTitle}{" "}
                        <span className="font-normal text-muted-foreground">
                          {entry.versionNumber}
                        </span>
                      </span>
                      <span className="block truncate font-mono text-xs text-muted-foreground">
                        {entry.fileName}
                        {entry.requiredBy ? ` · required by ${entry.requiredBy}` : ""}
                      </span>
                    </span>
                  </span>
                  {entry.alreadyInstalled ? (
                    <Badge>already installed</Badge>
                  ) : entry.size ? (
                    <span className="text-xs text-muted-foreground">
                      {formatBytes(Number(entry.size))}
                    </span>
                  ) : null}
                </li>
              ))}
            </ul>

            {plan.data.optional.length > 0 ? (
              <div className="rounded-md border border-border bg-muted/40 p-3">
                <p className="text-sm font-medium">Optional, not installed</p>
                <ul className="mt-1 list-inside list-disc text-xs text-muted-foreground">
                  {plan.data.optional.map((entry) => (
                    <li key={entry.projectId}>
                      {entry.projectTitle} — suggested by {entry.suggestedBy}
                    </li>
                  ))}
                </ul>
              </div>
            ) : null}

            <p className="text-xs text-muted-foreground">
              {toInstall.length} file{toInstall.length === 1 ? "" : "s"} to download
              {plan.data.totalSize ? ` · ${formatBytes(Number(plan.data.totalSize))}` : ""}
            </p>
          </div>
        ) : null}

        <DialogFooter>
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button disabled={!plan.data || toInstall.length === 0} onClick={() => void confirm()}>
            <Download /> Install {toInstall.length > 0 ? `${toInstall.length} file(s)` : ""}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
