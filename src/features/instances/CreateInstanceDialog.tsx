import { open } from "@tauri-apps/plugin-dialog";
import { AlertTriangle, FolderOpen } from "lucide-react";
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
import { JavaPlanNotice } from "@/features/setup/ManagedRuntimes";
import { useProviderBuilds } from "@/features/setup/queries";
import { errorMessage, ipc } from "@/lib/ipc";
import { SERVER_TYPES, SERVER_TYPE_LABEL } from "@/lib/status";
import type { ServerType } from "@/lib/types";
import { useAppInfo, useCreateInstance } from "./queries";
import { VersionTable } from "./VersionTable";

interface Props {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

/** Share of this machine's RAM above which a heap is called out. */
const RAM_WARNING_SHARE = 0.7;

export function CreateInstanceDialog({ open: isOpen, onOpenChange }: Props) {
  const { data: appInfo } = useAppInfo();
  const create = useCreateInstance();

  const [name, setName] = useState("");
  const [root, setRoot] = useState("");
  const [path, setPath] = useState("");
  const [serverType, setServerType] = useState<ServerType>("paper");
  const [mcVersion, setMcVersion] = useState("");
  const [build, setBuild] = useState("");
  const [maxRam, setMaxRam] = useState(4096);

  // Builds depend on both choices, so the dropdown only has something to show
  // once a version is picked. Vanilla has none at all.
  const builds = useProviderBuilds(serverType, mcVersion, Boolean(mcVersion));
  const buildRows = builds.data ?? [];
  const needsBuild = serverType !== "vanilla";

  useEffect(() => {
    if (isOpen && appInfo && !root) setRoot(appInfo.defaultInstanceRoot);
  }, [appInfo, isOpen, root]);

  // A version from one server type means nothing to another: Paper's list is
  // not Mojang's, and a stale selection would install the wrong thing.
  useEffect(() => {
    setMcVersion("");
    setBuild("");
  }, [serverType]);

  useEffect(() => setBuild(""), [mcVersion]);

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
    setBuild("");
    setServerType("paper");
    setMaxRam(4096);
  }

  async function submit(event: React.FormEvent) {
    event.preventDefault();
    if (!path) {
      toast.error("Pick a parent folder and a name first");
      return;
    }
    if (!mcVersion) {
      toast.error("Choose a Minecraft version");
      return;
    }
    try {
      await create.mutateAsync({
        name: name.trim(),
        path,
        serverType,
        mcVersion,
        loaderVersion: build || null,
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

  const totalRam = appInfo?.totalRamMb ?? 0;
  const ramIsGreedy = totalRam > 0 && maxRam > totalRam * RAM_WARNING_SHARE;

  return (
    <Dialog open={isOpen} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-2xl">
        <DialogHeader>
          <DialogTitle>New server</DialogTitle>
          <DialogDescription>
            Creates the folder and records the server. The files are downloaded in a later step,
            and the EULA is never accepted for you.
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

          {/* Server type first: it decides which versions exist. */}
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
            <Label>Minecraft version</Label>
            <VersionTable serverType={serverType} value={mcVersion} onChange={setMcVersion} />
          </div>

          {needsBuild ? (
            <div className="grid gap-2">
              <Label htmlFor="instance-build">
                {serverType === "fabric" ? "Loader version" : "Build"}
              </Label>
              <Select
                id="instance-build"
                value={build}
                disabled={!mcVersion || builds.isLoading || buildRows.length === 0}
                onChange={(event) => setBuild(event.target.value)}
              >
                <option value="">
                  {!mcVersion
                    ? "Choose a version first"
                    : builds.isLoading
                      ? "Loading…"
                      : buildRows.length === 0
                        ? "None published for this version"
                        : "Newest (recommended)"}
                </option>
                {buildRows.map((entry) => (
                  <option key={entry.id} value={entry.id}>
                    {entry.id}
                    {entry.label ? ` · ${entry.label}` : ""}
                  </option>
                ))}
              </Select>
            </div>
          ) : null}

          {/* Which Java this version needs, and the download that provides it —
              asked here rather than at the first failed start. */}
          <JavaPlanNotice mcVersion={mcVersion} serverType={serverType} />

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
            {ramIsGreedy ? (
              // Not a refusal: somebody with 16 GB and nothing else running may
              // well mean it. It is a warning because the failure it prevents -
              // the machine swapping until it stops responding - looks like the
              // server hanging rather than like a setting.
              <p className="flex items-start gap-2 text-xs text-[var(--status-starting)]">
                <AlertTriangle className="mt-0.5 size-3.5 shrink-0" aria-hidden />
                <span>
                  That is more than {Math.round(RAM_WARNING_SHARE * 100)}% of this computer's{" "}
                  {Math.round(totalRam / 1024)} GB. Leave room for Windows, the launcher and
                  anything else running, or the machine will swap and the server will stutter.
                </span>
              </p>
            ) : totalRam > 0 ? (
              <p className="text-xs text-muted-foreground">
                This computer has {Math.round(totalRam / 1024)} GB.
              </p>
            ) : null}
          </div>

          <DialogFooter>
            <Button type="button" variant="ghost" onClick={() => onOpenChange(false)}>
              Cancel
            </Button>
            <Button type="submit" disabled={create.isPending || !mcVersion}>
              {create.isPending ? "Creating…" : "Create server"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
