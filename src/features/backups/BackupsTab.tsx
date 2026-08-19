import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Archive, Clock, HardDriveDownload, Play, Trash2 } from "lucide-react";
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
import { Badge, Switch } from "@/components/ui/misc";
import { MetricsPanel } from "@/features/metrics/MetricsPanel";
import { onBackupsChanged, onTaskDone, onTaskProgress } from "@/lib/events";
import { formatBytes } from "@/lib/format";
import { ipc } from "@/lib/ipc";
import { toastError } from "@/lib/toast";
import type {
  Backup,
  BackupOptions,
  Format,
  InstanceView,
  MetricsWindow,
  Schedule,
  ScheduleInput,
  Scope,
} from "@/lib/types";

import {
  formatLabel,
  isSafetyCopy,
  kindLabel,
  scheduleSummary,
  scopeLabel,
  sortForDisplay,
  spaceWarning,
  whenLabel,
} from "./backupLabels";

export function BackupsTab({ instance }: { instance: InstanceView }) {
  const queryClient = useQueryClient();
  const [creating, setCreating] = useState(false);
  const [restoring, setRestoring] = useState<Backup | null>(null);
  const [editingSchedule, setEditingSchedule] = useState<Schedule | "new" | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [window, setWindow] = useState<MetricsWindow>("hour");

  const backups = useQuery({
    queryKey: ["backups", instance.id],
    queryFn: () => ipc.backupsList(instance.id),
  });

  const schedules = useQuery({
    queryKey: ["schedules", instance.id],
    queryFn: () => ipc.schedulesList(instance.id),
  });

  useEffect(() => {
    const pending = [
      onBackupsChanged((uuid) => {
        if (uuid !== instance.uuid) return;
        void queryClient.invalidateQueries({ queryKey: ["backups", instance.id] });
        void queryClient.invalidateQueries({ queryKey: ["schedules", instance.id] });
      }),
      onTaskProgress((payload) => {
        if (payload.instanceId !== instance.id) return;
        if (payload.kind !== "backup" && payload.kind !== "restore") return;
        setBusy(`${payload.kind === "backup" ? "Archiving" : "Restoring"}: ${payload.message}`);
      }),
      onTaskDone((payload) => {
        if (payload.instanceId !== instance.id) return;
        if (payload.kind !== "backup" && payload.kind !== "restore") return;
        setBusy(null);
        void queryClient.invalidateQueries({ queryKey: ["backups", instance.id] });
        if (payload.ok) {
          toast.success(payload.kind === "backup" ? "Backup written" : "Backup restored");
        } else if (payload.cancelled) {
          toast.message("Cancelled");
        } else {
          toast.error(payload.error ?? "That did not work");
        }
      }),
    ];
    return () => {
      pending.forEach((promise) => void promise.then((unlisten) => unlisten()));
    };
  }, [instance.id, instance.uuid, queryClient]);

  const remove = useMutation({
    mutationFn: (backup: Backup) => ipc.backupDelete(backup.id),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["backups", instance.id] });
      toast.success("Backup deleted");
    },
    onError: (error: unknown) => toastError(error),
  });

  const runSchedule = useMutation({
    mutationFn: (schedule: Schedule) => ipc.scheduleRunNow(schedule.id),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["backups", instance.id] });
      void queryClient.invalidateQueries({ queryKey: ["schedules", instance.id] });
      toast.success("Schedule run");
    },
    onError: (error: unknown) => toastError(error),
  });

  const deleteSchedule = useMutation({
    mutationFn: (schedule: Schedule) => ipc.scheduleDelete(schedule.id),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["schedules", instance.id] });
      toast.success("Schedule removed");
    },
    onError: (error: unknown) => toastError(error),
  });

  const rows = sortForDisplay(backups.data ?? []);

  return (
    <div className="flex min-h-0 flex-1 flex-col gap-6 overflow-y-auto py-4">
      <section className="space-y-3">
        <header className="flex flex-wrap items-center justify-between gap-2">
          <div>
            <h3 className="text-sm font-medium">Backups</h3>
            <p className="text-xs text-muted-foreground">
              {rows.length === 0
                ? "No backups yet."
                : `${rows.length} archive${rows.length === 1 ? "" : "s"}, ${formatBytes(
                    rows.reduce((total, backup) => total + backup.sizeBytes, 0),
                  )} on disk`}
            </p>
          </div>
          <Button onClick={() => setCreating(true)} disabled={busy !== null}>
            <Archive /> Back up now
          </Button>
        </header>

        {busy ? (
          <p className="rounded-md border border-border bg-muted/40 px-3 py-2 text-xs" role="status">
            {busy}
          </p>
        ) : null}

        <ul className="space-y-2">
          {rows.map((backup) => (
            <li
              key={backup.id}
              className="flex flex-wrap items-center justify-between gap-3 rounded-lg border border-border px-3 py-2"
            >
              <div className="min-w-0">
                <p className="truncate text-sm">{whenLabel(backup.createdAt)}</p>
                <p className="truncate text-xs text-muted-foreground">
                  {kindLabel(backup.kind)} · {scopeLabel(backup.scope)} ·{" "}
                  {formatLabel(backup.format)} · {formatBytes(backup.sizeBytes)}
                  {backup.label ? ` · ${backup.label}` : ""}
                </p>
              </div>
              <div className="flex items-center gap-2">
                {isSafetyCopy(backup) ? <Badge>Safety copy</Badge> : null}
                <Button
                  size="sm"
                  variant="outline"
                  onClick={() => setRestoring(backup)}
                  disabled={busy !== null}
                >
                  <HardDriveDownload /> Restore
                </Button>
                <Button
                  size="sm"
                  variant="ghost"
                  aria-label={`Delete the backup from ${whenLabel(backup.createdAt)}`}
                  onClick={() => remove.mutate(backup)}
                >
                  <Trash2 />
                </Button>
              </div>
            </li>
          ))}
        </ul>
      </section>

      <section className="space-y-3">
        <header className="flex items-center justify-between gap-2">
          <div>
            <h3 className="text-sm font-medium">Schedules</h3>
            <p className="text-xs text-muted-foreground">
              A schedule that came due while the app was closed runs once on the next start.
            </p>
          </div>
          <Button variant="outline" onClick={() => setEditingSchedule("new")}>
            <Clock /> Add schedule
          </Button>
        </header>

        <ul className="space-y-2">
          {(schedules.data ?? []).map((schedule) => (
            <li
              key={schedule.id}
              className="flex flex-wrap items-center justify-between gap-3 rounded-lg border border-border px-3 py-2"
            >
              <div className="min-w-0">
                <p className="truncate text-sm">{scheduleSummary(schedule)}</p>
                <p className="text-xs text-muted-foreground">
                  {schedule.lastRunAt ? `Last run ${whenLabel(schedule.lastRunAt)}` : "Never run"}
                  {schedule.nextRunAt ? ` · next ${whenLabel(schedule.nextRunAt)}` : ""}
                </p>
              </div>
              <div className="flex items-center gap-2">
                {schedule.enabled ? null : <Badge>Paused</Badge>}
                <Button
                  size="sm"
                  variant="ghost"
                  aria-label="Run this schedule now"
                  onClick={() => runSchedule.mutate(schedule)}
                  disabled={busy !== null}
                >
                  <Play />
                </Button>
                <Button size="sm" variant="outline" onClick={() => setEditingSchedule(schedule)}>
                  Edit
                </Button>
                <Button
                  size="sm"
                  variant="ghost"
                  aria-label="Remove this schedule"
                  onClick={() => deleteSchedule.mutate(schedule)}
                >
                  <Trash2 />
                </Button>
              </div>
            </li>
          ))}
        </ul>
      </section>

      <MetricsPanel instance={instance} window={window} onWindowChange={setWindow} />

      <CreateDialog
        instance={instance}
        open={creating}
        onOpenChange={setCreating}
        onStarted={() => setBusy("Preparing…")}
      />
      <RestoreDialog
        instance={instance}
        backup={restoring}
        onOpenChange={(open) => !open && setRestoring(null)}
        onStarted={() => setBusy("Preparing…")}
      />
      <ScheduleDialog
        instance={instance}
        schedule={editingSchedule}
        onOpenChange={(open) => !open && setEditingSchedule(null)}
      />
    </div>
  );
}

function CreateDialog({
  instance,
  open,
  onOpenChange,
  onStarted,
}: {
  instance: InstanceView;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onStarted: () => void;
}) {
  const [format, setFormat] = useState<Format>("tar_zst");
  const [scope, setScope] = useState<Scope>("full");
  const [level, setLevel] = useState<string>("");
  const [label, setLabel] = useState("");

  const options: BackupOptions = {
    format,
    scope,
    level: level === "" ? null : Number(level),
    label: label.trim() === "" ? null : label.trim(),
    exclude: [],
  };

  // The estimate and the free-space check are shown before anything is written.
  const check = useQuery({
    queryKey: ["backup-plan", instance.id, format, scope, level],
    queryFn: () => ipc.backupPlan(instance.id, options),
    enabled: open,
  });

  const start = useMutation({
    mutationFn: () => ipc.backupCreate(instance.id, options),
    onSuccess: () => {
      onStarted();
      onOpenChange(false);
    },
    onError: (error: unknown) => toastError(error),
  });

  const warning = spaceWarning(check.data);

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Back up "{instance.name}"</DialogTitle>
          <DialogDescription>
            A running server has saving paused for the copy and switched back on afterwards.
          </DialogDescription>
        </DialogHeader>

        <div className="grid gap-3">
          <div className="grid gap-1.5">
            <Label htmlFor="backup-scope">What to include</Label>
            <Select
              id="backup-scope"
              value={scope}
              onChange={(event) => setScope(event.target.value as Scope)}
            >
              <option value="full">Full instance</option>
              <option value="worlds">Worlds only</option>
            </Select>
          </div>

          <div className="grid gap-1.5">
            <Label htmlFor="backup-format">Format</Label>
            <Select
              id="backup-format"
              value={format}
              onChange={(event) => setFormat(event.target.value as Format)}
            >
              <option value="tar_zst">tar.zst — smaller and faster</option>
              <option value="zip">zip — opens anywhere</option>
            </Select>
          </div>

          <div className="grid gap-1.5">
            <Label htmlFor="backup-level">Compression level</Label>
            <Input
              id="backup-level"
              type="number"
              placeholder="Default"
              value={level}
              onChange={(event) => setLevel(event.target.value)}
            />
          </div>

          <div className="grid gap-1.5">
            <Label htmlFor="backup-label">Label (optional)</Label>
            <Input
              id="backup-label"
              value={label}
              placeholder="before the 1.21.4 update"
              onChange={(event) => setLabel(event.target.value)}
            />
          </div>

          <p className="text-xs text-muted-foreground">
            {check.data
              ? `About ${formatBytes(check.data.estimate.bytes)} across ${
                  check.data.estimate.files
                } files${
                  check.data.freeBytes === null
                    ? ""
                    : `, ${formatBytes(check.data.freeBytes)} free`
                }.`
              : "Measuring…"}
          </p>
          {warning ? (
            <p className="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-xs">
              {warning}
            </p>
          ) : null}
        </div>

        <DialogFooter>
          <Button variant="ghost" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button
            onClick={() => start.mutate()}
            disabled={start.isPending || warning !== null || !check.data}
          >
            Start backup
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function RestoreDialog({
  instance,
  backup,
  onOpenChange,
  onStarted,
}: {
  instance: InstanceView;
  backup: Backup | null;
  onOpenChange: (open: boolean) => void;
  onStarted: () => void;
}) {
  const [typed, setTyped] = useState("");

  useEffect(() => setTyped(""), [backup?.id]);

  const preview = useQuery({
    queryKey: ["backup-preview", backup?.id],
    queryFn: () => ipc.backupPreview(backup?.id ?? 0),
    enabled: backup !== null,
  });

  const start = useMutation({
    mutationFn: () => ipc.backupRestore(backup?.id ?? 0),
    onSuccess: () => {
      onStarted();
      onOpenChange(false);
    },
    onError: (error: unknown) => toastError(error),
  });

  const entries = preview.data ?? [];
  const confirmed = typed.trim() === instance.name;

  return (
    <Dialog open={backup !== null} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Restore into "{instance.name}"</DialogTitle>
          <DialogDescription>
            The instance folder is archived first, then replaced with the contents of this backup.
            The server has to be stopped.
          </DialogDescription>
        </DialogHeader>

        <div className="grid gap-3">
          <div className="max-h-48 overflow-y-auto rounded-md border border-border p-2 text-xs">
            {preview.isPending ? (
              <p className="text-muted-foreground">Reading the archive…</p>
            ) : entries.length === 0 ? (
              <p className="text-muted-foreground">This archive is empty.</p>
            ) : (
              <ul className="space-y-0.5">
                {entries.slice(0, 200).map((entry) => (
                  <li key={entry.path} className="flex justify-between gap-4">
                    <span className="truncate">{entry.path}</span>
                    <span className="shrink-0 text-muted-foreground">
                      {formatBytes(entry.size)}
                    </span>
                  </li>
                ))}
                {entries.length > 200 ? (
                  <li className="text-muted-foreground">
                    …and {entries.length - 200} more files
                  </li>
                ) : null}
              </ul>
            )}
          </div>

          <div className="grid gap-1.5">
            <Label htmlFor="restore-confirm">
              Type "{instance.name}" to confirm
            </Label>
            <Input
              id="restore-confirm"
              value={typed}
              autoComplete="off"
              onChange={(event) => setTyped(event.target.value)}
            />
          </div>
        </div>

        <DialogFooter>
          <Button variant="ghost" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button
            variant="destructive"
            disabled={!confirmed || start.isPending}
            onClick={() => start.mutate()}
          >
            Restore
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function ScheduleDialog({
  instance,
  schedule,
  onOpenChange,
}: {
  instance: InstanceView;
  schedule: Schedule | "new" | null;
  onOpenChange: (open: boolean) => void;
}) {
  const queryClient = useQueryClient();
  const existing = schedule === "new" || schedule === null ? null : schedule;

  const [cadence, setCadence] = useState<"interval" | "daily">("interval");
  const [minutes, setMinutes] = useState("360");
  const [time, setTime] = useState("04:00");
  const [scope, setScope] = useState<Scope>("full");
  const [format, setFormat] = useState<Format>("tar_zst");
  const [keepCount, setKeepCount] = useState("7");
  const [keepDays, setKeepDays] = useState("");
  const [enabled, setEnabled] = useState(true);
  const [restartAfter, setRestartAfter] = useState(false);
  const [skipIfIdle, setSkipIfIdle] = useState(true);

  useEffect(() => {
    if (!existing) return;
    setCadence(existing.intervalMinutes ? "interval" : "daily");
    setMinutes(String(existing.intervalMinutes ?? 360));
    setTime(existing.cron ?? "04:00");
    setScope(existing.scope);
    setFormat(existing.format);
    setKeepCount(existing.keepCount === null ? "" : String(existing.keepCount));
    setKeepDays(existing.keepDays === null ? "" : String(existing.keepDays));
    setEnabled(existing.enabled);
    setRestartAfter(existing.restartAfter);
    setSkipIfIdle(existing.skipIfIdle);
  }, [existing]);

  const save = useMutation({
    mutationFn: () => {
      const input: ScheduleInput = {
        id: existing?.id ?? null,
        cron: cadence === "daily" ? time : null,
        intervalMinutes: cadence === "interval" ? Number(minutes) : null,
        scope,
        format,
        compressionLevel: null,
        keepCount: keepCount === "" ? null : Number(keepCount),
        keepDays: keepDays === "" ? null : Number(keepDays),
        enabled,
        restartAfter,
        skipIfIdle,
      };
      return ipc.scheduleSave(instance.id, input);
    },
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["schedules", instance.id] });
      toast.success("Schedule saved");
      onOpenChange(false);
    },
    onError: (error: unknown) => toastError(error),
  });

  return (
    <Dialog open={schedule !== null} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{existing ? "Edit schedule" : "New schedule"}</DialogTitle>
          <DialogDescription>
            Retention runs straight after each backup. Manual backups are never pruned.
          </DialogDescription>
        </DialogHeader>

        <div className="grid gap-3">
          <div className="grid gap-1.5">
            <Label htmlFor="schedule-cadence">Cadence</Label>
            <Select
              id="schedule-cadence"
              value={cadence}
              onChange={(event) => setCadence(event.target.value as "interval" | "daily")}
            >
              <option value="interval">Every N minutes</option>
              <option value="daily">Daily at a time</option>
            </Select>
          </div>

          {cadence === "interval" ? (
            <div className="grid gap-1.5">
              <Label htmlFor="schedule-minutes">Minutes between runs</Label>
              <Input
                id="schedule-minutes"
                type="number"
                min={1}
                value={minutes}
                onChange={(event) => setMinutes(event.target.value)}
              />
            </div>
          ) : (
            <div className="grid gap-1.5">
              <Label htmlFor="schedule-time">Time of day (UTC, HH:MM)</Label>
              <Input
                id="schedule-time"
                value={time}
                onChange={(event) => setTime(event.target.value)}
              />
            </div>
          )}

          <div className="grid gap-1.5">
            <Label htmlFor="schedule-scope">What to include</Label>
            <Select
              id="schedule-scope"
              value={scope}
              onChange={(event) => setScope(event.target.value as Scope)}
            >
              <option value="full">Full instance</option>
              <option value="worlds">Worlds only</option>
            </Select>
          </div>

          <div className="grid gap-1.5">
            <Label htmlFor="schedule-format">Format</Label>
            <Select
              id="schedule-format"
              value={format}
              onChange={(event) => setFormat(event.target.value as Format)}
            >
              <option value="tar_zst">tar.zst</option>
              <option value="zip">zip</option>
            </Select>
          </div>

          <div className="grid grid-cols-2 gap-3">
            <div className="grid gap-1.5">
              <Label htmlFor="schedule-keep-count">Keep newest</Label>
              <Input
                id="schedule-keep-count"
                type="number"
                min={1}
                placeholder="No limit"
                value={keepCount}
                onChange={(event) => setKeepCount(event.target.value)}
              />
            </div>
            <div className="grid gap-1.5">
              <Label htmlFor="schedule-keep-days">Keep for days</Label>
              <Input
                id="schedule-keep-days"
                type="number"
                min={1}
                placeholder="No limit"
                value={keepDays}
                onChange={(event) => setKeepDays(event.target.value)}
              />
            </div>
          </div>

          <label className="flex items-center justify-between gap-3 text-sm">
            <span>
              Skip when nobody played
              <span className="block text-xs text-muted-foreground">
                Saves a pile of identical archives of an idle server.
              </span>
            </span>
            <Switch checked={skipIfIdle} onCheckedChange={setSkipIfIdle} />
          </label>

          <label className="flex items-center justify-between gap-3 text-sm">
            <span>Restart the server afterwards</span>
            <Switch checked={restartAfter} onCheckedChange={setRestartAfter} />
          </label>

          <label className="flex items-center justify-between gap-3 text-sm">
            <span>Enabled</span>
            <Switch checked={enabled} onCheckedChange={setEnabled} />
          </label>
        </div>

        <DialogFooter>
          <Button variant="ghost" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button onClick={() => save.mutate()} disabled={save.isPending}>
            Save schedule
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
