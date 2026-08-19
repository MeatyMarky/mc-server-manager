import { useQuery } from "@tanstack/react-query";
import { save as saveDialog } from "@tauri-apps/plugin-dialog";
import { ChevronDown, FileArchive } from "lucide-react";
import { useState } from "react";
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
import { Select } from "@/components/ui/input";
import { ErrorNotice } from "@/components/ui/ErrorNotice";
import { formatBytes } from "@/lib/format";
import { ipc } from "@/lib/ipc";
import { toastError } from "@/lib/toast";
import type { InstanceView, ReportPart } from "@/lib/types";

const LINE_CHOICES = [100, 500, 2000];

/**
 * Builds the file a user attaches to a bug report.
 *
 * The whole point is that they read it first: every part is shown in full,
 * expandable, before anything is written — and nothing is uploaded by the app
 * at any point.
 */
export function ReportProblemDialog({
  open,
  onOpenChange,
  instance,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  instance: InstanceView | null;
}) {
  const [lines, setLines] = useState(500);
  const [saving, setSaving] = useState(false);

  const preview = useQuery({
    queryKey: ["report-preview", instance?.id ?? null, lines],
    queryFn: () => ipc.reportPreview(instance?.id ?? null, lines),
    enabled: open,
  });

  async function write() {
    const suggested = preview.data?.suggestedName ?? "msm-report.zip";
    const target = await saveDialog({
      title: "Save the problem report",
      defaultPath: suggested,
      filters: [{ name: "Zip archive", extensions: ["zip"] }],
    });
    if (!target) return;

    setSaving(true);
    try {
      const written = await ipc.reportWrite(target, instance?.id ?? null, lines);
      toast.success("Report saved", { description: written });
      onOpenChange(false);
    } catch (error) {
      toastError(error);
    } finally {
      setSaving(false);
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-2xl">
        <DialogHeader>
          <DialogTitle>Report a problem</DialogTitle>
          <DialogDescription>
            This writes a zip you can attach to an issue. Read what is in it first — the app
            never sends anything anywhere.
          </DialogDescription>
        </DialogHeader>

        <div className="flex flex-wrap items-end justify-between gap-3">
          <div className="grid gap-1.5">
            <Label htmlFor="report-lines">Lines of log and console</Label>
            <Select
              id="report-lines"
              value={String(lines)}
              onChange={(event) => setLines(Number(event.target.value))}
            >
              {LINE_CHOICES.map((choice) => (
                <option key={choice} value={choice}>
                  Last {choice}
                </option>
              ))}
            </Select>
          </div>
          <p className="text-xs text-muted-foreground">
            {instance
              ? `Includes the console of "${instance.name}".`
              : "No server selected, so no console is included."}
          </p>
        </div>

        {preview.error ? <ErrorNotice error={preview.error} /> : null}

        <div className="max-h-80 space-y-2 overflow-y-auto">
          {(preview.data?.parts ?? []).map((part) => (
            <PartRow key={part.name} part={part} />
          ))}
        </div>

        <p className="rounded-md border border-border bg-muted/40 p-3 text-xs text-muted-foreground">
          {preview.data?.notice}
        </p>

        <DialogFooter>
          <span className="mr-auto text-xs text-muted-foreground">
            {preview.data ? `About ${formatBytes(preview.data.totalBytes)} before compression` : ""}
          </span>
          <Button variant="ghost" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button onClick={() => void write()} disabled={!preview.data || saving}>
            <FileArchive /> Save report…
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function PartRow({ part }: { part: ReportPart }) {
  const [open, setOpen] = useState(false);

  return (
    <div className="rounded-md border border-border">
      <button
        type="button"
        className="flex w-full items-start gap-2 p-2 text-left focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring"
        aria-expanded={open}
        onClick={() => setOpen((current) => !current)}
      >
        <ChevronDown
          className={`mt-0.5 size-4 shrink-0 transition-transform ${open ? "rotate-180" : ""}`}
          aria-hidden
        />
        <span className="min-w-0">
          <span className="block font-mono text-xs">{part.name}</span>
          <span className="block text-xs text-muted-foreground">{part.purpose}</span>
        </span>
        <span className="ml-auto shrink-0 text-xs text-muted-foreground">
          {formatBytes(part.content.length)}
        </span>
      </button>

      {open ? (
        <pre className="max-h-56 overflow-auto border-t border-border bg-background/60 p-2 text-xs whitespace-pre-wrap">
          {part.content}
        </pre>
      ) : null}
    </div>
  );
}
