import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { RotateCw, Save, Search, TriangleAlert } from "lucide-react";
import { useMemo, useState } from "react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/dialog";
import { Input, Select } from "@/components/ui/input";
import { Badge, Switch } from "@/components/ui/misc";
import { ipc } from "@/lib/ipc";
import { toastError } from "@/lib/toast";
import type { InstanceView, PropertyEntry } from "@/lib/types";
import { cn } from "@/lib/utils";

/** Matches a key, its value or its description, so search finds what people mean. */
export function matchesSearch(entry: PropertyEntry, search: string): boolean {
  const needle = search.trim().toLowerCase();
  if (!needle) return true;
  return (
    entry.key.toLowerCase().includes(needle) ||
    entry.value.toLowerCase().includes(needle) ||
    entry.info.description.toLowerCase().includes(needle)
  );
}

export function ConfigTab({ instance }: { instance: InstanceView }) {
  const queryClient = useQueryClient();
  const [search, setSearch] = useState("");
  const [edits, setEdits] = useState<Record<string, string>>({});

  const properties = useQuery({
    queryKey: ["properties", instance.id],
    queryFn: () => ipc.propertiesRead(instance.id),
  });

  const save = useMutation({
    mutationFn: () => ipc.propertiesWrite(instance.id, { changes: edits }),
    onSuccess: (report) => {
      setEdits({});
      void queryClient.invalidateQueries({ queryKey: ["properties", instance.id] });
      void queryClient.invalidateQueries({ queryKey: ["worlds", instance.id] });
      if (report.changed.length === 0) {
        toast.message("Nothing to save");
        return;
      }
      toast.success(`Saved ${report.changed.length} change${report.changed.length === 1 ? "" : "s"}`, {
        description: report.restartRequired
          ? "The server re-reads server.properties only on start — restart to apply."
          : report.backupCreated
            ? "The original file was kept as server.properties.orig."
            : undefined,
      });
    },
    onError: (error: unknown) => toastError(error),
  });

  const restart = useMutation({
    mutationFn: () => ipc.instanceRestart(instance.id),
    onSuccess: () => toast.success("Restarting to apply the changes"),
    onError: (error: unknown) => toastError(error),
  });

  const entries = properties.data?.entries ?? [];
  const visible = useMemo(
    () => entries.filter((entry) => matchesSearch(entry, search)),
    [entries, search],
  );
  const grouped = useMemo(() => {
    const groups = new Map<string, PropertyEntry[]>();
    for (const entry of visible) {
      const list = groups.get(entry.info.group) ?? [];
      list.push(entry);
      groups.set(entry.info.group, list);
    }
    return [...groups.entries()].sort(([a], [b]) => a.localeCompare(b));
  }, [visible]);

  const pending = Object.keys(edits).length;
  const value = (entry: PropertyEntry) => edits[entry.key] ?? entry.value;
  const setValue = (key: string, next: string) =>
    setEdits((current) => ({ ...current, [key]: next }));

  if (properties.isLoading) {
    return <p className="text-sm text-muted-foreground">Reading server.properties…</p>;
  }

  if (properties.data && !properties.data.exists) {
    return (
      <div className="rounded-lg border border-dashed border-border p-8 text-center">
        <h3 className="text-sm font-semibold">No server.properties yet</h3>
        <p className="mx-auto mt-2 max-w-md text-sm text-muted-foreground">
          The server writes this file the first time it starts. Start the server once, then
          come back to edit it.
        </p>
      </div>
    );
  }

  return (
    <div className="flex h-full min-h-0 flex-col gap-4">
      <div className="flex flex-wrap items-center gap-2">
        <div className="relative min-w-48 flex-1">
          <Search className="pointer-events-none absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground" />
          <Input
            className="h-8 pl-8 text-xs"
            placeholder="Search settings"
            aria-label="Search settings"
            value={search}
            onChange={(event) => setSearch(event.target.value)}
          />
        </div>
        {pending > 0 ? <Badge>{pending} unsaved</Badge> : null}
        <Button size="sm" disabled={pending === 0 || save.isPending} onClick={() => save.mutate()}>
          <Save /> {save.isPending ? "Saving…" : "Save changes"}
        </Button>
      </div>

      {properties.data?.running ? (
        <div className="flex flex-wrap items-center justify-between gap-3 rounded-md border border-border bg-muted/40 p-3">
          <p className="flex items-center gap-2 text-sm">
            <TriangleAlert className="size-4 text-[var(--status-starting)]" />
            The server is running and read this file at startup. Changes take effect after a
            restart.
          </p>
          <Button
            size="sm"
            variant="outline"
            disabled={restart.isPending}
            onClick={() => restart.mutate()}
          >
            <RotateCw /> Restart to apply
          </Button>
        </div>
      ) : null}

      <div className="min-h-0 flex-1 overflow-y-auto pr-1">
        {grouped.length === 0 ? (
          <p className="py-6 text-center text-sm text-muted-foreground">
            No setting matches that search.
          </p>
        ) : (
          grouped.map(([group, groupEntries]) => (
            <section key={group} className="mb-6">
              <h3 className="mb-3 text-sm font-semibold">{group}</h3>
              <div className="grid gap-4">
                {groupEntries.map((entry) => (
                  <PropertyRow
                    key={entry.key}
                    entry={entry}
                    value={value(entry)}
                    dirty={entry.key in edits}
                    onChange={(next) => setValue(entry.key, next)}
                  />
                ))}
              </div>
            </section>
          ))
        )}
      </div>

      {properties.data?.backupExists ? (
        <p className="text-xs text-muted-foreground">
          The file as it was before the first edit is kept as{" "}
          <span className="font-mono">server.properties.orig</span>.
        </p>
      ) : null}
    </div>
  );
}

function PropertyRow({
  entry,
  value,
  dirty,
  onChange,
}: {
  entry: PropertyEntry;
  value: string;
  dirty: boolean;
  onChange: (value: string) => void;
}) {
  const id = `property-${entry.key}`;
  const kind = entry.info.kind;

  return (
    <div className={cn("grid gap-1.5 rounded-md border border-transparent", dirty && "border-border bg-muted/30 p-3")}>
      <div className="flex flex-wrap items-center gap-2">
        <Label htmlFor={id} className="font-mono text-xs">
          {entry.key}
        </Label>
        {entry.info.known ? null : <Badge>unknown key</Badge>}
        {dirty ? <Badge>changed</Badge> : null}
      </div>

      {kind.kind === "bool" ? (
        <div className="flex items-center gap-3">
          <Switch
            id={id}
            checked={value === "true"}
            onCheckedChange={(checked) => onChange(checked ? "true" : "false")}
          />
          <span className="text-xs text-muted-foreground">{value}</span>
        </div>
      ) : kind.kind === "int" ? (
        <Input
          id={id}
          type="number"
          className="max-w-40"
          min={kind.min ?? undefined}
          max={kind.max ?? undefined}
          value={value}
          onChange={(event) => onChange(event.target.value)}
        />
      ) : kind.kind === "enum" ? (
        <Select
          id={id}
          className="max-w-72"
          value={value}
          onChange={(event) => onChange(event.target.value)}
        >
          {/* A value the server accepts but this build has not heard of stays selectable. */}
          {kind.options.includes(value) ? null : <option value={value}>{value}</option>}
          {kind.options.map((option) => (
            <option key={option} value={option}>
              {option}
            </option>
          ))}
        </Select>
      ) : (
        <Input
          id={id}
          value={value}
          className="font-mono text-xs"
          onChange={(event) => onChange(event.target.value)}
        />
      )}

      <p className="text-xs text-muted-foreground">{entry.info.description}</p>
    </div>
  );
}
