import { open } from "@tauri-apps/plugin-dialog";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  Ban,
  Boxes,
  Download,
  Package,
  Pin,
  PinOff,
  RefreshCw,
  Search,
  Trash2,
  TriangleAlert,
} from "lucide-react";
import { useEffect, useState } from "react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Badge, Switch } from "@/components/ui/misc";
import { onTaskDone, onTaskProgress } from "@/lib/events";
import { formatBytes } from "@/lib/format";
import { errorMessage, ipc } from "@/lib/ipc";
import type { InstanceView, ModView, Project } from "@/lib/types";
import { InstallPlanDialog } from "./InstallPlanDialog";
import { contentLabel, displayName, displayVersion, mismatchSummary, sortForDisplay } from "./modLabels";
import { PackImportDialog } from "./PackImportDialog";

export function ModsTab({ instance }: { instance: InstanceView }) {
  const queryClient = useQueryClient();
  const [search, setSearch] = useState("");
  const [results, setResults] = useState<Project[] | null>(null);
  const [searching, setSearching] = useState(false);
  const [planFor, setPlanFor] = useState<Project | null>(null);
  const [pack, setPack] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);

  const mods = useQuery({
    queryKey: ["mods", instance.id],
    queryFn: () => ipc.modsList(instance.id),
  });

  useEffect(() => {
    const unlisteners = [
      onTaskProgress((payload) => {
        if (payload.instanceId !== instance.id) return;
        if (payload.kind === "mod_install" || payload.kind === "mrpack_import") {
          setBusy(
            payload.total
              ? `${payload.message} (${payload.done}/${payload.total})`
              : payload.message,
          );
        }
      }),
      onTaskDone((payload) => {
        if (payload.instanceId !== instance.id) return;
        if (payload.kind !== "mod_install" && payload.kind !== "mrpack_import") return;

        setBusy(null);
        void queryClient.invalidateQueries({ queryKey: ["mods", instance.id] });
        if (payload.ok) {
          toast.success(payload.kind === "mrpack_import" ? "Pack imported" : "Install finished");
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

  const setEnabled = useMutation({
    mutationFn: ({ fileName, enabled }: { fileName: string; enabled: boolean }) =>
      ipc.modsSetEnabled(instance.id, fileName, enabled),
    onSuccess: (view) => queryClient.setQueryData(["mods", instance.id], view),
    onError: (error: unknown) => toast.error(errorMessage(error)),
  });

  const setPinned = useMutation({
    mutationFn: ({ fileName, pinned }: { fileName: string; pinned: boolean }) =>
      ipc.modsSetPinned(instance.id, fileName, pinned),
    onSuccess: (view) => queryClient.setQueryData(["mods", instance.id], view),
    onError: (error: unknown) => toast.error(errorMessage(error)),
  });

  const uninstall = useMutation({
    mutationFn: (fileName: string) => ipc.modsUninstall(instance.id, fileName),
    onSuccess: (dependents, fileName) => {
      void queryClient.invalidateQueries({ queryKey: ["mods", instance.id] });
      if (dependents.length > 0) {
        toast.warning(`Removed ${fileName}`, {
          description: `${dependents.join(", ")} depended on it and may stop working.`,
        });
      } else {
        toast.success(`Removed ${fileName}`);
      }
    },
    onError: (error: unknown) => toast.error(errorMessage(error)),
  });

  const checkUpdates = useMutation({
    mutationFn: () => ipc.modsCheckUpdates(instance.id),
    onSuccess: (view) => {
      queryClient.setQueryData(["mods", instance.id], view);
      const updates = view.mods.filter((mod) => mod.tracked?.updateVersionId).length;
      toast.success(updates === 0 ? "Everything is up to date" : `${updates} update(s) available`);
    },
    onError: (error: unknown) => toast.error(errorMessage(error)),
  });

  async function runSearch(event: React.FormEvent) {
    event.preventDefault();
    setSearching(true);
    try {
      setResults(await ipc.modsSearch(instance.id, search, 20));
    } catch (error) {
      toast.error(errorMessage(error));
    } finally {
      setSearching(false);
    }
  }

  async function addLocalJar() {
    const picked = await open({
      title: "Choose a jar",
      filters: [{ name: "Jar", extensions: ["jar"] }],
    });
    if (typeof picked !== "string") return;
    try {
      const installed = await ipc.modsInstallLocal(instance.id, picked);
      void queryClient.invalidateQueries({ queryKey: ["mods", instance.id] });
      if (installed.mismatch) {
        toast.warning(`Installed ${installed.fileName}`, {
          description: [installed.mismatch.loader, installed.mismatch.gameVersion]
            .filter(Boolean)
            .join(" · "),
        });
      } else {
        toast.success(`Installed ${installed.fileName}`);
      }
    } catch (error) {
      toast.error(errorMessage(error));
    }
  }

  async function choosePack() {
    const picked = await open({
      title: "Import a Modrinth pack",
      filters: [{ name: "Modrinth pack", extensions: ["mrpack"] }],
    });
    if (typeof picked === "string") setPack(picked);
  }

  // Vanilla loads nothing, and the backend says so in a sentence worth showing.
  if (mods.data?.unsupported) {
    return (
      <div className="rounded-lg border border-dashed border-border p-8 text-center">
        <Ban className="mx-auto mb-3 size-6 text-muted-foreground" />
        <h3 className="text-sm font-semibold">No mods or plugins here</h3>
        <p className="mx-auto mt-2 max-w-md text-sm text-muted-foreground">
          {mods.data.unsupported}
        </p>
      </div>
    );
  }

  const label = contentLabel(mods.data?.loader ?? null);

  return (
    <div className="flex h-full min-h-0 flex-col gap-4">
      <form className="flex flex-wrap items-center gap-2" onSubmit={runSearch}>
        <div className="relative min-w-48 flex-1">
          <Search className="pointer-events-none absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground" />
          <Input
            className="h-8 pl-8 text-xs"
            placeholder={`Search Modrinth for ${label.toLowerCase()} (${mods.data?.loader ?? ""} ${instance.mcVersion})`}
            aria-label="Search Modrinth"
            value={search}
            onChange={(event) => setSearch(event.target.value)}
          />
        </div>
        <Button type="submit" size="sm" disabled={searching || !search.trim()}>
          {searching ? "Searching…" : "Search"}
        </Button>
        <Button type="button" size="sm" variant="outline" onClick={() => void addLocalJar()}>
          <Package /> Add jar…
        </Button>
        <Button type="button" size="sm" variant="outline" onClick={() => void choosePack()}>
          <Boxes /> Import .mrpack…
        </Button>
        <Button
          type="button"
          size="sm"
          variant="outline"
          disabled={checkUpdates.isPending}
          onClick={() => checkUpdates.mutate()}
        >
          <RefreshCw /> {checkUpdates.isPending ? "Checking…" : "Check updates"}
        </Button>
      </form>

      {busy ? (
        <p className="rounded-md border border-border bg-muted/40 px-3 py-2 text-xs text-muted-foreground">
          {busy}
        </p>
      ) : null}

      <div className="min-h-0 flex-1 overflow-y-auto pr-1">
        {results ? (
          <section className="mb-6">
            <div className="mb-2 flex items-center justify-between">
              <h3 className="text-sm font-semibold">Search results</h3>
              <Button size="sm" variant="ghost" onClick={() => setResults(null)}>
                Clear
              </Button>
            </div>

            {results.length === 0 ? (
              <p className="text-sm text-muted-foreground">
                Nothing matched for {mods.data?.loader} on {instance.mcVersion}.
              </p>
            ) : (
              <ul className="grid gap-2">
                {results.map((project) => (
                  <li
                    key={project.id}
                    className="flex items-start justify-between gap-3 rounded-md border border-border p-3"
                  >
                    <div className="min-w-0">
                      <p className="text-sm font-medium">{project.title}</p>
                      <p className="line-clamp-2 text-xs text-muted-foreground">
                        {project.description}
                      </p>
                      <p className="mt-1 text-xs text-muted-foreground">
                        {project.author ? `${project.author} · ` : ""}
                        {project.downloads
                          ? `${Number(project.downloads).toLocaleString()} downloads`
                          : ""}
                      </p>
                    </div>
                    <Button size="sm" onClick={() => setPlanFor(project)}>
                      <Download /> Install
                    </Button>
                  </li>
                ))}
              </ul>
            )}
          </section>
        ) : null}

        <section>
          <h3 className="mb-2 text-sm font-semibold">
            Installed {label.toLowerCase()}{" "}
            <span className="text-xs font-normal text-muted-foreground">
              in {mods.data?.contentDir}/
            </span>
          </h3>

          {mods.isLoading ? (
            <p className="text-sm text-muted-foreground">Reading the folder…</p>
          ) : (mods.data?.mods.length ?? 0) === 0 ? (
            <p className="text-sm text-muted-foreground">
              Nothing installed yet. Search above, add a jar, or import a pack.
            </p>
          ) : (
            <ul className="grid gap-2">
              {sortForDisplay(mods.data?.mods ?? []).map((mod) => (
                <ModRow
                  key={mod.fileName}
                  mod={mod}
                  onToggle={(enabled) =>
                    setEnabled.mutate({ fileName: mod.fileName, enabled })
                  }
                  onPin={(pinned) => setPinned.mutate({ fileName: mod.fileName, pinned })}
                  onRemove={() => uninstall.mutate(mod.fileName)}
                />
              ))}
            </ul>
          )}
        </section>
      </div>

      <InstallPlanDialog
        instance={instance}
        project={planFor}
        onClose={() => setPlanFor(null)}
      />
      <PackImportDialog instance={instance} archive={pack} onClose={() => setPack(null)} />
    </div>
  );
}

function ModRow({
  mod,
  onToggle,
  onPin,
  onRemove,
}: {
  mod: ModView;
  onToggle: (enabled: boolean) => void;
  onPin: (pinned: boolean) => void;
  onRemove: () => void;
}) {
  const tracked = mod.tracked;
  const title = displayName(mod);
  const version = displayVersion(mod);

  return (
    <li className="rounded-md border border-border p-3">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="min-w-0">
          <p className="flex flex-wrap items-center gap-2 text-sm font-medium">
            {title}
            {version ? <Badge>{version}</Badge> : null}
            {mod.enabled ? null : <Badge>disabled</Badge>}
            {tracked?.updateVersionId ? <Badge>update available</Badge> : null}
            {tracked?.pinned ? <Badge>pinned</Badge> : null}
          </p>
          <p className="truncate font-mono text-xs text-muted-foreground">
            {mod.fileName} · {formatBytes(Number(mod.sizeBytes))}
            {mod.metadata ? ` · ${mod.metadata.format}` : " · no metadata"}
          </p>

          {mod.mismatch ? (
            <p className="mt-1 flex items-start gap-1 text-xs text-[var(--status-starting)]">
              <TriangleAlert className="mt-0.5 size-3.5 shrink-0" />
              <span>{mismatchSummary(mod)}</span>
            </p>
          ) : null}

          {mod.requiredBy.length > 0 ? (
            <p className="mt-1 text-xs text-muted-foreground">
              Required by {mod.requiredBy.join(", ")}
            </p>
          ) : null}
        </div>

        <div className="flex items-center gap-2">
          <Switch
            checked={mod.enabled}
            aria-label={mod.enabled ? "Disable" : "Enable"}
            onCheckedChange={onToggle}
          />
          {tracked ? (
            <Button
              size="sm"
              variant="ghost"
              title={tracked.pinned ? "Unpin to allow updates" : "Pin to this version"}
              onClick={() => onPin(!tracked.pinned)}
            >
              {tracked.pinned ? <PinOff /> : <Pin />}
            </Button>
          ) : null}
          <Button size="sm" variant="ghost" className="text-destructive" onClick={onRemove}>
            <Trash2 />
          </Button>
        </div>
      </div>
    </li>
  );
}
