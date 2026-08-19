import { open } from "@tauri-apps/plugin-dialog";
import { FolderOpen, Info } from "lucide-react";
import { useState } from "react";

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
import { Badge } from "@/components/ui/misc";
import { Input, Select } from "@/components/ui/input";
import { errorMessage, ipc } from "@/lib/ipc";
import { toastError } from "@/lib/toast";
import { SERVER_TYPES, SERVER_TYPE_LABEL } from "@/lib/status";
import type { ImportCandidate, ServerType } from "@/lib/types";
import { useImportInstance } from "./queries";

interface Props {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

/**
 * Two steps: pick a folder, then confirm what detection found. Detection itself
 * runs in Rust and never writes anything.
 */
export function ImportInstanceDialog({ open: isOpen, onOpenChange }: Props) {
  const importInstance = useImportInstance();
  const [candidate, setCandidate] = useState<ImportCandidate | null>(null);
  const [detecting, setDetecting] = useState(false);
  const [name, setName] = useState("");
  const [serverType, setServerType] = useState<ServerType>("paper");
  const [mcVersion, setMcVersion] = useState("");

  async function pickFolder() {
    const picked = await open({ directory: true, title: "Choose an existing server folder" });
    if (typeof picked !== "string") return;

    setDetecting(true);
    try {
      const found = await ipc.instanceImportDetect(picked);
      setCandidate(found);
      setName(found.suggestedName);
      setServerType(found.serverType);
      setMcVersion(found.mcVersion ?? "");
    } catch (error) {
      toastError(error);
      setCandidate(null);
    } finally {
      setDetecting(false);
    }
  }

  function close() {
    setCandidate(null);
    setName("");
    setMcVersion("");
    onOpenChange(false);
  }

  async function submit(event: React.FormEvent) {
    event.preventDefault();
    if (!candidate) return;
    try {
      await importInstance.mutateAsync({
        path: candidate.path,
        name: name.trim(),
        serverType,
        mcVersion: mcVersion.trim(),
        loaderVersion: candidate.loaderVersion,
      });
      close();
    } catch (error) {
      console.error(errorMessage(error));
    }
  }

  return (
    <Dialog open={isOpen} onOpenChange={(next) => (next ? onOpenChange(true) : close())}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Import an existing server</DialogTitle>
          <DialogDescription>
            Point at a folder that already holds a server. Nothing in it is modified apart
            from adding a small <code>.msm</code> metadata folder.
          </DialogDescription>
        </DialogHeader>

        {!candidate ? (
          <div className="grid gap-4 py-2">
            <Button type="button" variant="outline" onClick={pickFolder} disabled={detecting}>
              <FolderOpen /> {detecting ? "Inspecting…" : "Choose folder…"}
            </Button>
          </div>
        ) : (
          <form className="grid gap-4" onSubmit={submit}>
            <div className="rounded-md border border-border bg-muted/40 p-3 text-xs">
              <p className="mb-2 flex items-center gap-2 font-medium">
                <Info className="size-3.5" />
                Detected
                <Badge>{candidate.confidence} confidence</Badge>
                {candidate.fromManifest ? <Badge>previously managed</Badge> : null}
              </p>
              <p className="truncate text-muted-foreground" title={candidate.path}>
                {candidate.path}
              </p>
              <ul className="mt-2 list-inside list-disc text-muted-foreground">
                {candidate.notes.map((note) => (
                  <li key={note}>{note}</li>
                ))}
                {candidate.worlds.length > 0 ? (
                  <li>Worlds: {candidate.worlds.join(", ")}</li>
                ) : null}
                <li>EULA already accepted: {candidate.eulaAccepted ? "yes" : "no"}</li>
              </ul>
            </div>

            <div className="grid gap-2">
              <Label htmlFor="import-name">Name</Label>
              <Input
                id="import-name"
                required
                value={name}
                onChange={(event) => setName(event.target.value)}
              />
            </div>

            <div className="grid grid-cols-2 gap-4">
              <div className="grid gap-2">
                <Label htmlFor="import-type">Server type</Label>
                <Select
                  id="import-type"
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
                <Label htmlFor="import-version">Minecraft version</Label>
                <Input
                  id="import-version"
                  required
                  placeholder="1.21.4"
                  value={mcVersion}
                  onChange={(event) => setMcVersion(event.target.value)}
                />
              </div>
            </div>

            <DialogFooter>
              <Button type="button" variant="ghost" onClick={() => setCandidate(null)}>
                Back
              </Button>
              <Button type="submit" disabled={importInstance.isPending}>
                {importInstance.isPending ? "Importing…" : "Import instance"}
              </Button>
            </DialogFooter>
          </form>
        )}
      </DialogContent>
    </Dialog>
  );
}
