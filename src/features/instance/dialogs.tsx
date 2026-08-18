import { open } from "@tauri-apps/plugin-dialog";
import { FolderOpen } from "lucide-react";
import { useEffect, useState } from "react";

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
import { Switch } from "@/components/ui/misc";
import { ipc } from "@/lib/ipc";
import type { InstanceView } from "@/lib/types";
import {
  useCloneInstance,
  useDeleteInstance,
  useLocateInstance,
  useRenameInstance,
} from "@/features/instances/queries";

interface DialogProps {
  instance: InstanceView;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

export function RenameDialog({ instance, open: isOpen, onOpenChange }: DialogProps) {
  const rename = useRenameInstance();
  const [name, setName] = useState(instance.name);

  useEffect(() => setName(instance.name), [instance.name, isOpen]);

  return (
    <Dialog open={isOpen} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-md">
        <DialogHeader>
          <DialogTitle>Rename instance</DialogTitle>
          <DialogDescription>
            Only the display name changes. The folder on disk keeps its current name.
          </DialogDescription>
        </DialogHeader>
        <form
          className="grid gap-4"
          onSubmit={async (event) => {
            event.preventDefault();
            await rename.mutateAsync({ id: instance.id, name: name.trim() }).catch(() => {});
            onOpenChange(false);
          }}
        >
          <div className="grid gap-2">
            <Label htmlFor="rename-name">Name</Label>
            <Input
              id="rename-name"
              autoFocus
              required
              value={name}
              onChange={(event) => setName(event.target.value)}
            />
          </div>
          <DialogFooter>
            <Button type="button" variant="ghost" onClick={() => onOpenChange(false)}>
              Cancel
            </Button>
            <Button type="submit" disabled={rename.isPending}>
              Rename
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

export function CloneDialog({ instance, open: isOpen, onOpenChange }: DialogProps) {
  const clone = useCloneInstance();
  const [name, setName] = useState(`${instance.name} copy`);
  const [root, setRoot] = useState("");
  const [path, setPath] = useState("");
  const [includeWorlds, setIncludeWorlds] = useState(true);

  useEffect(() => {
    if (isOpen) setName(`${instance.name} copy`);
  }, [instance.name, isOpen]);

  // Rust builds the destination path; the dialog only shows it.
  useEffect(() => {
    if (!isOpen || !root || !name.trim()) {
      setPath("");
      return;
    }
    let cancelled = false;
    ipc
      .instanceSuggestPath(root, name)
      .then((suggested) => !cancelled && setPath(suggested))
      .catch(() => !cancelled && setPath(""));
    return () => {
      cancelled = true;
    };
  }, [isOpen, name, root]);

  return (
    <Dialog open={isOpen} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Clone "{instance.name}"</DialogTitle>
          <DialogDescription>
            Copies configuration and content. Logs, caches and lock files are left behind.
          </DialogDescription>
        </DialogHeader>
        <form
          className="grid gap-4"
          onSubmit={async (event) => {
            event.preventDefault();
            if (!path) return;
            await clone
              .mutateAsync({ sourceId: instance.id, name: name.trim(), path, includeWorlds })
              .catch(() => {});
            onOpenChange(false);
          }}
        >
          <div className="grid gap-2">
            <Label htmlFor="clone-name">New name</Label>
            <Input
              id="clone-name"
              required
              value={name}
              onChange={(event) => setName(event.target.value)}
            />
          </div>
          <div className="grid gap-2">
            <Label htmlFor="clone-root">Parent folder</Label>
            <div className="flex gap-2">
              <Input
                id="clone-root"
                required
                value={root}
                onChange={(event) => setRoot(event.target.value)}
              />
              <Button
                type="button"
                variant="outline"
                size="icon"
                aria-label="Browse"
                onClick={async () => {
                  const picked = await open({ directory: true, title: "Choose a parent folder" });
                  if (typeof picked === "string") setRoot(picked);
                }}
              >
                <FolderOpen />
              </Button>
            </div>
            {path ? (
              <p className="truncate text-xs text-muted-foreground" title={path}>
                Folder: {path}
              </p>
            ) : null}
          </div>
          <div className="flex items-center justify-between rounded-md border border-border p-3">
            <div>
              <Label htmlFor="clone-worlds">Copy worlds</Label>
              <p className="text-xs text-muted-foreground">
                Turn off for the same setup with a fresh map.
              </p>
            </div>
            <Switch id="clone-worlds" checked={includeWorlds} onCheckedChange={setIncludeWorlds} />
          </div>
          <DialogFooter>
            <Button type="button" variant="ghost" onClick={() => onOpenChange(false)}>
              Cancel
            </Button>
            <Button type="submit" disabled={clone.isPending || !path}>
              {clone.isPending ? "Copying…" : "Clone"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

export function DeleteDialog({ instance, open: isOpen, onOpenChange }: DialogProps) {
  const remove = useDeleteInstance();
  const [deleteFiles, setDeleteFiles] = useState(false);
  const [confirmation, setConfirmation] = useState("");

  useEffect(() => {
    if (isOpen) {
      setDeleteFiles(false);
      setConfirmation("");
    }
  }, [isOpen]);

  const confirmed = !deleteFiles || confirmation.trim() === instance.name;

  return (
    <Dialog open={isOpen} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-md">
        <DialogHeader>
          <DialogTitle>Delete "{instance.name}"?</DialogTitle>
          <DialogDescription>
            The instance is removed from the list. Its files are only deleted if you ask for
            it below — that cannot be undone.
          </DialogDescription>
        </DialogHeader>

        <div className="grid gap-4">
          <div className="flex items-center justify-between rounded-md border border-border p-3">
            <div>
              <Label htmlFor="delete-files">Also delete the folder</Label>
              <p className="max-w-64 truncate text-xs text-muted-foreground" title={instance.path}>
                {instance.path}
              </p>
            </div>
            <Switch id="delete-files" checked={deleteFiles} onCheckedChange={setDeleteFiles} />
          </div>

          {deleteFiles ? (
            <div className="grid gap-2">
              <Label htmlFor="delete-confirm">
                Type <span className="font-mono">{instance.name}</span> to confirm
              </Label>
              <Input
                id="delete-confirm"
                value={confirmation}
                onChange={(event) => setConfirmation(event.target.value)}
              />
            </div>
          ) : null}
        </div>

        <DialogFooter>
          <Button type="button" variant="ghost" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button
            variant="destructive"
            disabled={!confirmed || remove.isPending}
            onClick={async () => {
              await remove.mutateAsync({ id: instance.id, deleteFiles }).catch(() => {});
              onOpenChange(false);
            }}
          >
            {deleteFiles ? "Delete instance and files" : "Remove from list"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

/** Recovery for a `missing` instance: repoint it at the folder it moved to. */
export function LocateBanner({ instance }: { instance: InstanceView }) {
  const locate = useLocateInstance();

  return (
    <div className="flex flex-wrap items-center justify-between gap-3 rounded-md border border-border bg-muted/50 p-3">
      <div className="min-w-0">
        <p className="text-sm font-medium">This instance's folder is missing</p>
        <p className="truncate text-xs text-muted-foreground" title={instance.path}>
          Expected at {instance.path}
        </p>
      </div>
      <Button
        size="sm"
        variant="outline"
        disabled={locate.isPending}
        onClick={async () => {
          const picked = await open({
            directory: true,
            title: `Where is "${instance.name}" now?`,
          });
          if (typeof picked === "string") {
            await locate.mutateAsync({ id: instance.id, path: picked }).catch(() => {});
          }
        }}
      >
        <FolderOpen /> Locate folder…
      </Button>
    </div>
  );
}
