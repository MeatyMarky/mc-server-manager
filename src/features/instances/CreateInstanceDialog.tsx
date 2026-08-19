import { open } from "@tauri-apps/plugin-dialog";
import { FolderOpen } from "lucide-react";
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
import { Input, Select } from "@/components/ui/input";
import { errorMessage, ipc } from "@/lib/ipc";
import { JavaPlanNotice } from "@/features/setup/ManagedRuntimes";
import { SERVER_TYPES, SERVER_TYPE_LABEL } from "@/lib/status";
import type { ServerType } from "@/lib/types";
import { useAppInfo, useCreateInstance } from "./queries";

interface Props {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

export function CreateInstanceDialog({ open: isOpen, onOpenChange }: Props) {
  const { data: appInfo } = useAppInfo();
  const create = useCreateInstance();

  const [name, setName] = useState("");
  const [root, setRoot] = useState("");
  const [path, setPath] = useState("");
  const [serverType, setServerType] = useState<ServerType>("paper");
  const [mcVersion, setMcVersion] = useState("");
  const [maxRam, setMaxRam] = useState(4096);

  useEffect(() => {
    if (isOpen && appInfo && !root) setRoot(appInfo.defaultInstanceRoot);
  }, [appInfo, isOpen, root]);

  // The folder name is derived in Rust, never assembled here.
  useEffect(() => {
    if (!isOpen || !root || !name.trim()) {
      setPath("");
      return;
    }
    let cancelled = false;
    const timer = window.setTimeout(() => {
      ipc
        .instanceSuggestPath(root, name)
        .then((suggested) => {
          if (!cancelled) setPath(suggested);
        })
        .catch(() => {
          if (!cancelled) setPath("");
        });
    }, 150);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [isOpen, name, root]);

  async function pickRoot() {
    const picked = await open({ directory: true, title: "Choose where the instance folder goes" });
    if (typeof picked === "string") setRoot(picked);
  }

  function reset() {
    setName("");
    setPath("");
    setMcVersion("");
    setServerType("paper");
    setMaxRam(4096);
  }

  async function submit(event: React.FormEvent) {
    event.preventDefault();
    if (!path) {
      toast.error("Pick a parent folder and a name first");
      return;
    }
    try {
      await create.mutateAsync({
        name: name.trim(),
        path,
        serverType,
        mcVersion: mcVersion.trim(),
        loaderVersion: null,
        minRamMb: Math.min(1024, maxRam),
        maxRamMb: maxRam,
        notes: null,
        color: null,
      });
      reset();
      onOpenChange(false);
    } catch (error) {
      // The mutation already toasted; keep the dialog open so the user can fix it.
      console.error(errorMessage(error));
    }
  }

  return (
    <Dialog open={isOpen} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>New instance</DialogTitle>
          <DialogDescription>
            Creates the folder and records the instance. The server jar is downloaded in a
            later step, and the EULA is never accepted for you.
          </DialogDescription>
        </DialogHeader>

        <form className="grid gap-4" onSubmit={submit}>
          <div className="grid gap-2">
            <Label htmlFor="instance-name">Name</Label>
            <Input
              id="instance-name"
              autoFocus
              required
              value={name}
              placeholder="Survival 1.21"
              onChange={(event) => setName(event.target.value)}
            />
          </div>

          <div className="grid gap-2">
            <Label htmlFor="instance-root">Parent folder</Label>
            <div className="flex gap-2">
              <Input
                id="instance-root"
                required
                value={root}
                onChange={(event) => setRoot(event.target.value)}
              />
              <Button type="button" variant="outline" size="icon" onClick={pickRoot} aria-label="Browse">
                <FolderOpen />
              </Button>
            </div>
            {path ? (
              <p className="truncate text-xs text-muted-foreground" title={path}>
                Folder: {path}
              </p>
            ) : null}
          </div>

          <div className="grid grid-cols-2 gap-4">
            <div className="grid gap-2">
              <Label htmlFor="instance-type">Server type</Label>
              <Select
                id="instance-type"
                value={serverType}
                onChange={(event) => setServerType(event.target.value as ServerType)}
              >
                {SERVER_TYPES.map((type) => (
                  <option key={type} value={type}>
                    {SERVER_TYPE_LABEL[type]}
                  </option>
                ))}
              </Select>
            </div>
            <div className="grid gap-2">
              <Label htmlFor="instance-version">Minecraft version</Label>
              <Input
                id="instance-version"
                required
                value={mcVersion}
                placeholder="1.21.4"
                onChange={(event) => setMcVersion(event.target.value)}
              />
            </div>
          </div>

          {/* Which Java this version needs, and the download that provides it —
              asked here rather than at the first failed start. */}
          <JavaPlanNotice mcVersion={mcVersion} />

          <div className="grid gap-2">
            <Label htmlFor="instance-ram">Maximum RAM (MB)</Label>
            <Input
              id="instance-ram"
              type="number"
              min={512}
              step={512}
              value={maxRam}
              onChange={(event) => setMaxRam(Number(event.target.value))}
            />
          </div>

          <DialogFooter>
            <Button type="button" variant="ghost" onClick={() => onOpenChange(false)}>
              Cancel
            </Button>
            <Button type="submit" disabled={create.isPending}>
              {create.isPending ? "Creating…" : "Create instance"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
