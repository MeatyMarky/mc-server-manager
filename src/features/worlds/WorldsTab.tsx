import { save as saveDialog, open as openDialog } from "@tauri-apps/plugin-dialog";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Check, Download, HardDrive, Trash2, Upload } from "lucide-react";
import { useEffect, useState } from "react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  Label,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Badge } from "@/components/ui/misc";
import { onTaskDone, onTaskProgress } from "@/lib/events";
import { formatBytes } from "@/lib/format";
import { errorMessage, ipc } from "@/lib/ipc";
import type { InstanceView, World } from "@/lib/types";

export function WorldsTab({ instance }: { instance: InstanceView }) {
  const queryClient = useQueryClient();
  const [sizes, setSizes] = useState<Record<string, number>>({});
  const [measuring, setMeasuring] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState<string | null>(null);
  const [deleting, setDeleting] = useState<World | null>(null);

  const worlds = useQuery({
    queryKey: ["worlds", instance.id],
    queryFn: () => ipc.worldsList(instance.id),
  });

  // Sizing, exporting and importing all report through the task events.
  useEffect(() => {
    const unlisteners = [
      onTaskProgress((payload) => {
        if (payload.instanceId !== instance.id) return;
        if (payload.kind === "measure") {
          setMeasuring((current) => ({ ...current, [payload.taskId]: payload.message }));
          setSizes((current) => ({ ...current, [payload.taskId]: payload.done }));
        } else {
          setBusy(
            payload.total
              ? `${payload.message} (${payload.done}/${payload.total})`
              : payload.message,
          );
        }
      }),
      onTaskDone((payload) => {
        if (payload.instanceId !== instance.id) return;
        if (payload.kind === "measure") {
          setMeasuring((current) => {
            const next = { ...current };
            delete next[payload.taskId];
            return next;
          });
          if (payload.ok && payload.logTail) {
            setSizes((current) => ({ ...current, [payload.taskId]: Number(payload.logTail) }));
          }
          return;
        }

        setBusy(null);
        void queryClient.invalidateQueries({ queryKey: ["worlds", instance.id] });
        if (payload.ok) {
          toast.success(payload.kind === "world_export" ? "World exported" : "World imported");
        } else if (payload.cancelled) {
          toast.message("Cancelled");
        } else {
          toast.error(payload.error ?? "That did not work");
        }
      }),
    ];
    return () => {
      unlisteners.forEach((pending) => void pending.then((unlisten) => unlisten()));
    };
  }, [instance.id, queryClient]);

  const switchWorld = useMutation({
    mutationFn: (folder: string) => ipc.worldSwitch(instance.id, folder),
    onSuccess: (_result, folder) => {
      void queryClient.invalidateQueries({ queryKey: ["worlds", instance.id] });
      void queryClient.invalidateQueries({ queryKey: ["properties", instance.id] });
      toast.success(`"${folder}" is now the active world`);
    },
    onError: (error: unknown) => toast.error(errorMessage(error)),
  });

  const remove = useMutation({
    mutationFn: (folder: string) => ipc.worldDelete(instance.id, folder),
    onSuccess: (_result, folder) => {
      setDeleting(null);
      void queryClient.invalidateQueries({ queryKey: ["worlds", instance.id] });
      toast.success(`Deleted "${folder}"`);
    },
    onError: (error: unknown) => toast.error(errorMessage(error)),
  });

  // Track which task belongs to which world, so its size lands on the right row.
  const [taskWorld, setTaskWorld] = useState<Record<string, string>>({});
  const sizeOf = (folder: string) => {
    const entry = Object.entries(taskWorld).find(([, name]) => name === folder);
    return entry ? sizes[entry[0]] : undefined;
  };
  const isMeasuring = (folder: string) =>
    Object.entries(taskWorld).some(([task, name]) => name === folder && task in measuring);

  async function measure(folder: string) {
    try {
      const taskId = await ipc.worldMeasure(instance.id, folder);
      setTaskWorld((current) => ({ ...current, [taskId]: folder }));
      setMeasuring((current) => ({ ...current, [taskId]: "Measuring…" }));
    } catch (error) {
      toast.error(errorMessage(error));
    }
  }

  async function exportWorld(folder: string) {
    const target = await saveDialog({
      title: `Export ${folder}`,
      defaultPath: `${folder}.zip`,
      filters: [{ name: "Zip archive", extensions: ["zip"] }],
    });
    if (typeof target !== "string") return;
    try {
      setBusy("Preparing…");
      await ipc.worldExport(instance.id, folder, target);
    } catch (error) {
      setBusy(null);
      toast.error(errorMessage(error));
    }
  }

  async function importWorld() {
    const archive = await openDialog({
      title: "Import a world",
      filters: [{ name: "Zip archive", extensions: ["zip"] }],
    });
    if (typeof archive !== "string") return;
    try {
      setBusy("Preparing…");
      await ipc.worldImport(instance.id, archive, null);
    } catch (error) {
      setBusy(null);
      toast.error(errorMessage(error));
    }
  }

  const running = instance.status !== "stopped" && instance.status !== "crashed";

  return (
    <div className="flex h-full min-h-0 flex-col gap-4">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <p className="text-sm text-muted-foreground">
          {running
            ? "The server is running: worlds can be exported, but switching and deleting need it stopped."
            : "Worlds in this instance folder."}
        </p>
        <Button size="sm" variant="outline" onClick={() => void importWorld()} disabled={busy !== null}>
          <Upload /> Import world…
        </Button>
      </div>

      {busy ? (
        <p className="rounded-md border border-border bg-muted/40 px-3 py-2 text-xs text-muted-foreground">
          {busy}
        </p>
      ) : null}

      <div className="min-h-0 flex-1 overflow-y-auto pr-1">
        {worlds.isLoading ? (
          <p className="text-sm text-muted-foreground">Scanning for worlds…</p>
        ) : (worlds.data?.length ?? 0) === 0 ? (
          <div className="rounded-lg border border-dashed border-border p-8 text-center">
            <h3 className="text-sm font-semibold">No worlds yet</h3>
            <p className="mx-auto mt-2 max-w-md text-sm text-muted-foreground">
              A world appears once the server has generated one, or after you import a zip.
            </p>
          </div>
        ) : (
          <ul className="grid gap-3">
            {worlds.data?.map((world) => (
              <li key={world.folder} className="rounded-lg border border-border p-4">
                <div className="flex flex-wrap items-start justify-between gap-3">
                  <div className="min-w-0">
                    <p className="flex flex-wrap items-center gap-2">
                      <span className="font-mono text-sm font-medium">{world.folder}</span>
                      {world.active ? <Badge>active</Badge> : null}
                      {world.hardcore ? <Badge>hardcore</Badge> : null}
                      {world.gameType ? <Badge>{world.gameType}</Badge> : null}
                    </p>
                    <p className="mt-1 text-xs text-muted-foreground">
                      {world.displayName && world.displayName !== world.folder
                        ? `“${world.displayName}” · `
                        : ""}
                      {world.version ? `${world.version} · ` : ""}
                      {world.seed !== null ? `seed ${world.seed} · ` : ""}
                      {world.lastPlayed
                        ? `last played ${new Date(Number(world.lastPlayed)).toLocaleString()}`
                        : "never played"}
                    </p>
                    {world.problem ? (
                      <p className="mt-1 text-xs text-destructive">{world.problem}</p>
                    ) : null}
                  </div>

                  <div className="flex flex-wrap gap-1">
                    {world.active ? null : (
                      <Button
                        size="sm"
                        variant="outline"
                        disabled={running || switchWorld.isPending}
                        title={running ? "Stop the server to switch worlds" : undefined}
                        onClick={() => switchWorld.mutate(world.folder)}
                      >
                        <Check /> Make active
                      </Button>
                    )}
                    <Button size="sm" variant="ghost" onClick={() => void measure(world.folder)}>
                      <HardDrive />
                      {isMeasuring(world.folder)
                        ? "Measuring…"
                        : sizeOf(world.folder) !== undefined
                          ? formatBytes(sizeOf(world.folder) ?? 0)
                          : "Size"}
                    </Button>
                    <Button
                      size="sm"
                      variant="ghost"
                      disabled={busy !== null}
                      onClick={() => void exportWorld(world.folder)}
                    >
                      <Download /> Export
                    </Button>
                    <Button
                      size="sm"
                      variant="ghost"
                      className="text-destructive"
                      disabled={running || world.active}
                      title={
                        world.active
                          ? "The active world cannot be deleted"
                          : running
                            ? "Stop the server first"
                            : undefined
                      }
                      onClick={() => setDeleting(world)}
                    >
                      <Trash2 /> Delete
                    </Button>
                  </div>
                </div>
              </li>
            ))}
          </ul>
        )}
      </div>

      <DeleteWorldDialog
        world={deleting}
        pending={remove.isPending}
        onCancel={() => setDeleting(null)}
        onConfirm={(folder) => remove.mutate(folder)}
      />
    </div>
  );
}

function DeleteWorldDialog({
  world,
  pending,
  onCancel,
  onConfirm,
}: {
  world: World | null;
  pending: boolean;
  onCancel: () => void;
  onConfirm: (folder: string) => void;
}) {
  const [confirmation, setConfirmation] = useState("");

  useEffect(() => setConfirmation(""), [world]);

  return (
    <Dialog open={world !== null} onOpenChange={(open) => (open ? null : onCancel())}>
      <DialogContent className="max-w-md">
        <DialogHeader>
          <DialogTitle>Delete “{world?.folder}”?</DialogTitle>
          <DialogDescription>
            The folder and everything in it is removed. This cannot be undone — export it
            first if you might want it back.
          </DialogDescription>
        </DialogHeader>

        <div className="grid gap-2">
          <Label htmlFor="confirm-world">
            Type <span className="font-mono">{world?.folder}</span> to confirm
          </Label>
          <Input
            id="confirm-world"
            autoFocus
            value={confirmation}
            onChange={(event) => setConfirmation(event.target.value)}
          />
        </div>

        <DialogFooter>
          <Button variant="ghost" onClick={onCancel}>
            Cancel
          </Button>
          <Button
            variant="destructive"
            disabled={pending || confirmation !== world?.folder}
            onClick={() => world && onConfirm(world.folder)}
          >
            Delete world
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
