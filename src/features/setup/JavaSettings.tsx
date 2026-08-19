import { open } from "@tauri-apps/plugin-dialog";
import { FolderOpen, RefreshCw } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/dialog";
import { Select } from "@/components/ui/input";
import { Badge } from "@/components/ui/misc";
import { useUpdateInstance } from "@/features/instances/queries";
import type { InstanceView } from "@/lib/types";
import { isUsable, unsuitableReason } from "./javaLabels";
import { useAddJava, useJavaRuntimes, useJavaStatus, useRescanJava } from "./queries";

/**
 * Java for one instance: what is installed, what this instance uses, and the
 * "browse for a JDK" fallback for runtimes detection cannot see.
 */
export function JavaSettings({ instance }: { instance: InstanceView }) {
  const runtimes = useJavaRuntimes();
  const status = useJavaStatus(instance.id);
  const rescan = useRescanJava();
  const addJava = useAddJava();
  const update = useUpdateInstance();

  const pinned = instance.javaPath ?? "";

  async function browse() {
    const picked = await open({
      directory: true,
      title: "Choose a JDK folder (or its bin folder)",
    });
    if (typeof picked === "string") addJava.mutate(picked);
  }

  return (
    <section className="grid gap-4">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h3 className="text-sm font-semibold">Java</h3>
        <div className="flex gap-2">
          <Button
            type="button"
            variant="outline"
            size="sm"
            disabled={rescan.isPending}
            onClick={() => rescan.mutate()}
          >
            <RefreshCw /> {rescan.isPending ? "Scanning…" : "Rescan"}
          </Button>
          <Button type="button" variant="outline" size="sm" onClick={() => void browse()}>
            <FolderOpen /> Browse for JDK…
          </Button>
        </div>
      </div>

      {status.data ? (
        <p className="flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
          <Badge>needs Java {status.data.requiredMajor}</Badge>
          {status.data.selected ? (
            <span>
              using Java {status.data.selected.major}
              {status.data.selected.vendor ? ` (${status.data.selected.vendor})` : ""}
            </span>
          ) : (
            <span className="text-destructive">nothing installed satisfies that</span>
          )}
        </p>
      ) : null}

      <div className="grid gap-2">
        <Label htmlFor="java-pin">Java for this instance</Label>
        <Select
          id="java-pin"
          value={pinned}
          onChange={(event) =>
            update.mutate({
              id: instance.id,
              input: {
                name: null,
                mcVersion: null,
                loaderVersion: null,
                javaPath: event.target.value ? event.target.value : null,
                jvmArgs: null,
                serverArgs: null,
                minRamMb: null,
                maxRamMb: null,
                autoStart: null,
                autoRestart: null,
                restartMax: null,
                restartWindowS: null,
                stopTimeoutS: null,
                notes: null,
                color: null,
              },
            })
          }
        >
          <option value="">Automatic (best match)</option>
          {runtimes.data?.map((runtime) => (
            <option
              key={runtime.id}
              value={runtime.path}
              // A 32-bit runtime stays visible so the list matches what is
              // installed, but it cannot be chosen: it refuses the heap a
              // server needs.
              disabled={!isUsable(runtime)}
            >
              Java {runtime.major}
              {runtime.vendor ? ` · ${runtime.vendor}` : ""} · {runtime.path}
              {isUsable(runtime) ? "" : ` — ${unsuitableReason(runtime)}`}
            </option>
          ))}
        </Select>
        {pinned && status.data && !status.data.pinnedValid ? (
          <p className="text-xs text-destructive">
            The pinned runtime is missing. Pick another one or rescan.
          </p>
        ) : null}
      </div>

      {status.data?.message ? (
        <p className="text-xs text-destructive">{status.data.message}</p>
      ) : null}

      {runtimes.data?.some((runtime) => !isUsable(runtime)) ? (
        <p className="text-xs text-muted-foreground">
          Greyed entries are 32-bit runtimes. They cannot address the memory a server is given,
          so they are never chosen automatically.
        </p>
      ) : null}

      {runtimes.data && runtimes.data.length === 0 ? (
        <p className="text-xs text-muted-foreground">
          No Java found yet. Rescan, or browse for a JDK folder.
        </p>
      ) : null}
    </section>
  );
}
