import { FolderSearch, Plus, Search, ServerCog } from "lucide-react";
import { useMemo, useState } from "react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { STATUS_COLOR, STATUS_LABEL, SERVER_TYPE_LABEL, statusPulses } from "@/lib/status";
import type { InstanceStatus, InstanceView } from "@/lib/types";
import { cn } from "@/lib/utils";

/// Decorative on purpose: every place that shows the dot also states the status
/// in text next to it, so announcing it here would read it out twice.
export function StatusDot({ status }: { status: InstanceStatus }) {
  return (
    <span
      aria-hidden
      className={cn(
        "inline-block size-2.5 shrink-0 rounded-full",
        statusPulses(status) && "animate-pulse",
      )}
      style={{ backgroundColor: STATUS_COLOR[status] }}
    />
  );
}

interface Props {
  instances: InstanceView[];
  isLoading: boolean;
  selectedId: number | null;
  onSelect: (id: number) => void;
  onCreate: () => void;
  onImport: () => void;
}

export function InstanceSidebar({
  instances,
  isLoading,
  selectedId,
  onSelect,
  onCreate,
  onImport,
}: Props) {
  const [filter, setFilter] = useState("");

  const visible = useMemo(() => {
    const needle = filter.trim().toLowerCase();
    if (!needle) return instances;
    return instances.filter(
      (instance) =>
        instance.name.toLowerCase().includes(needle) ||
        instance.mcVersion.toLowerCase().includes(needle) ||
        instance.serverType.toLowerCase().includes(needle),
    );
  }, [filter, instances]);

  return (
    <aside className="flex w-72 shrink-0 flex-col border-r border-border bg-card/40">
      <div className="flex items-center gap-2 px-3 py-3">
        <ServerCog className="size-5 text-primary" />
        <h1 className="text-sm font-semibold">Server Manager</h1>
      </div>

      <div className="flex gap-2 px-3 pb-3">
        <Button size="sm" className="flex-1" onClick={onCreate}>
          <Plus /> New
        </Button>
        <Button size="sm" variant="outline" className="flex-1" onClick={onImport}>
          <FolderSearch /> Import
        </Button>
      </div>

      <div className="relative px-3 pb-2">
        <Search className="pointer-events-none absolute left-5 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground" />
        <Input
          className="h-8 pl-7 text-xs"
          placeholder="Filter instances"
          aria-label="Filter instances"
          value={filter}
          onChange={(event) => setFilter(event.target.value)}
        />
      </div>

      <nav
        aria-label="Instances"
        className="min-h-0 flex-1 overflow-y-auto overscroll-contain px-2 pb-3"
      >
        {isLoading ? (
          <p className="px-2 py-6 text-center text-xs text-muted-foreground">Loading…</p>
        ) : visible.length === 0 ? (
          <EmptyState hasInstances={instances.length > 0} />
        ) : (
          <ul className="flex flex-col gap-1">
            {visible.map((instance) => (
              <li key={instance.id}>
                <button
                  type="button"
                  onClick={() => onSelect(instance.id)}
                  aria-current={instance.id === selectedId ? "true" : undefined}
                  className={cn(
                    "flex w-full items-center gap-2 rounded-md px-2 py-2 text-left transition-colors hover:bg-accent",
                    instance.id === selectedId && "bg-accent",
                    instance.status === "missing" && "opacity-60",
                  )}
                >
                  <StatusDot status={instance.status} />
                  <span className="min-w-0 flex-1">
                    <span className="block truncate text-sm font-medium">{instance.name}</span>
                    <span className="block truncate text-xs text-muted-foreground">
                      {SERVER_TYPE_LABEL[instance.serverType]} · {instance.mcVersion}
                    </span>
                  </span>
                  <span className="sr-only">{STATUS_LABEL[instance.status]}</span>
                </button>
              </li>
            ))}
          </ul>
        )}
      </nav>
    </aside>
  );
}

function EmptyState({ hasInstances }: { hasInstances: boolean }) {
  return (
    <p className="px-3 py-6 text-center text-xs text-muted-foreground">
      {hasInstances
        ? "No instance matches that filter."
        : "No instances yet. Create one, or import a folder you already have."}
    </p>
  );
}
