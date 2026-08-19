import { useQuery } from "@tanstack/react-query";
import { Boxes, TriangleAlert } from "lucide-react";

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
import type { InstanceView } from "@/lib/types";

/**
 * Shows what importing a `.mrpack` would do — including which files are skipped
 * because the pack marks them client-only, which is the difference between a
 * server that boots and one that crashes on startup.
 */
export function PackImportDialog({
  instance,
  archive,
  onClose,
}: {
  instance: InstanceView;
  archive: string | null;
  onClose: () => void;
}) {
  const plan = useQuery({
    queryKey: ["mrpack-plan", instance.id, archive],
    queryFn: () => ipc.mrpackPlan(instance.id, archive!),
    enabled: archive !== null,
    retry: false,
  });

  async function confirm() {
    if (!archive) return;
    try {
      await ipc.mrpackImport(instance.id, archive);
      onClose();
    } catch (error) {
      toastError(error);
    }
  }

  return (
    <Dialog open={archive !== null} onOpenChange={(open) => (open ? null : onClose())}>
      <DialogContent className="max-w-2xl">
        <DialogHeader>
          <DialogTitle>Import {plan.data?.index.name ?? "a modpack"}</DialogTitle>
          <DialogDescription>
            Every file is downloaded and its checksum verified into a staging folder first;
            the instance is only touched once the whole pack is ready.
          </DialogDescription>
        </DialogHeader>

        {plan.isLoading ? (
          <p className="text-sm text-muted-foreground">Reading the pack…</p>
        ) : plan.isError ? (
          <div className="rounded-md border border-destructive/50 bg-destructive/10 p-3">
            <p className="text-sm text-destructive">{errorMessage(plan.error)}</p>
          </div>
        ) : plan.data ? (
          <div className="grid gap-4">
            <div className="flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
              <Badge>Minecraft {plan.data.index.mcVersion}</Badge>
              {plan.data.index.loader ? <Badge>{plan.data.index.loader}</Badge> : null}
              {plan.data.index.versionId ? <Badge>{plan.data.index.versionId}</Badge> : null}
              <span>
                {plan.data.installCount} file(s) ·{" "}
                {formatBytes(Number(plan.data.totalSize))}
              </span>
            </div>

            {plan.data.index.summary ? (
              <p className="text-sm text-muted-foreground">{plan.data.index.summary}</p>
            ) : null}

            {plan.data.mismatch ? (
              <p className="flex items-start gap-2 rounded-md border border-border bg-muted/40 p-3 text-sm">
                <TriangleAlert className="mt-0.5 size-4 shrink-0 text-[var(--status-starting)]" />
                {plan.data.mismatch}
              </p>
            ) : null}

            {plan.data.skippedClientOnly.length > 0 ? (
              <div className="rounded-md border border-border p-3">
                <p className="text-sm font-medium">
                  {plan.data.skippedClientOnly.length} client-only file(s) will be skipped
                </p>
                <p className="mt-1 text-xs text-muted-foreground">
                  The pack marks these as unsupported on a server; installing them would crash
                  it.
                </p>
                <ul className="mt-2 max-h-32 overflow-y-auto font-mono text-xs text-muted-foreground">
                  {plan.data.skippedClientOnly.map((path) => (
                    <li key={path}>{path}</li>
                  ))}
                </ul>
              </div>
            ) : null}
          </div>
        ) : null}

        <DialogFooter>
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button disabled={!plan.data} onClick={() => void confirm()}>
            <Boxes /> Import pack
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
