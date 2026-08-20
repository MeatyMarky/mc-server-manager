import { Copy, FileText } from "lucide-react";
import { useState } from "react";

import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { copyToClipboard } from "@/lib/clipboard";
import { ipc } from "@/lib/ipc";
import { toastError } from "@/lib/toast";
import type { TaskDoneEvent } from "@/lib/types";

/**
 * When the Forge/NeoForge installer fails, the installer's own output is what
 * explains why. This shows it instead of a generic error, with the full log one
 * click away.
 */
export function InstallerFailureDialog({
  failure,
  onClose,
}: {
  failure: TaskDoneEvent | null;
  onClose: () => void;
}) {
  const [fullLog, setFullLog] = useState<string | null>(null);

  const shown = fullLog ?? failure?.logTail ?? "";

  async function loadFullLog() {
    if (!failure?.logPath) return;
    try {
      setFullLog(await ipc.readInstallerLog(failure.logPath));
    } catch (error) {
      toastError(error);
    }
  }

  return (
    <Dialog
      open={failure !== null}
      onOpenChange={(open) => {
        if (!open) {
          setFullLog(null);
          onClose();
        }
      }}
    >
      <DialogContent className="max-w-3xl">
        <DialogHeader>
          <DialogTitle>The installer failed — here's what it said</DialogTitle>
          <DialogDescription>
            {failure?.error ?? "The installer exited with an error."} The instance was left
            untouched, so retrying is safe.
          </DialogDescription>
        </DialogHeader>

        <pre className="max-h-80 overflow-auto rounded-md border border-border bg-muted/40 p-3 font-mono text-xs leading-relaxed">
          {shown || "The installer produced no output."}
        </pre>

        {failure?.logPath ? (
          <p className="truncate text-xs text-muted-foreground" title={failure.logPath}>
            Full log: {failure.logPath}
          </p>
        ) : null}

        <DialogFooter>
          <Button
            variant="ghost"
            onClick={() => void copyToClipboard(shown, "Installer log")}
          >
            <Copy /> Copy
          </Button>
          {fullLog === null && failure?.logPath ? (
            <Button variant="outline" onClick={() => void loadFullLog()}>
              <FileText /> Show full log
            </Button>
          ) : null}
          <Button
            onClick={() => {
              setFullLog(null);
              onClose();
            }}
          >
            Close
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
