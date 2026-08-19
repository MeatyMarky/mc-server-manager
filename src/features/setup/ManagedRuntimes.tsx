import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Download, HardDrive, Trash2 } from "lucide-react";
import { useEffect, useState } from "react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/misc";
import { onTaskDone, onTaskProgress } from "@/lib/events";
import { formatBytes, progressPercent } from "@/lib/format";
import { ipc } from "@/lib/ipc";
import { toastError } from "@/lib/toast";
import type { ManagedRuntime } from "@/lib/types";

/**
 * JDKs this app downloaded, so a server does not depend on what happens to be
 * installed on the machine.
 *
 * One runtime per Java version, shared by every instance that needs it — the
 * list is short by design, and each row says who would break if it went.
 */
export function ManagedRuntimes() {
  const queryClient = useQueryClient();
  const [busy, setBusy] = useState<string | null>(null);

  const runtimes = useQuery({
    queryKey: ["managed-runtimes"],
    queryFn: () => ipc.managedRuntimes(),
  });

  const total = useQuery({
    queryKey: ["managed-runtimes-size"],
    queryFn: () => ipc.managedRuntimesSize(),
  });

  const settings = useQuery({
    queryKey: ["settings"],
    queryFn: () => ipc.settingsGetAll(),
  });

  const systemOnly = settings.data?.use_system_java_only === "true";

  useEffect(() => {
    const pending = [
      onTaskProgress((payload) => {
        if (payload.kind !== "java_download") return;
        const percent = progressPercent(payload.done, payload.total ?? null);
        setBusy(
          percent === null
            ? payload.message
            : `${payload.message} — ${percent}% of ${formatBytes(payload.total ?? 0)}`,
        );
      }),
      onTaskDone((payload) => {
        if (payload.kind !== "java_download") return;
        setBusy(null);
        void queryClient.invalidateQueries({ queryKey: ["managed-runtimes"] });
        void queryClient.invalidateQueries({ queryKey: ["managed-runtimes-size"] });
        void queryClient.invalidateQueries({ queryKey: ["java"] });
        void queryClient.invalidateQueries({ queryKey: ["java-status"], exact: false });
        if (payload.ok) {
          toast.success("Java installed", { description: payload.logTail ?? undefined });
        } else if (payload.cancelled) {
          toast.message("Download cancelled");
        } else {
          toast.error(payload.error ?? "The download did not finish");
        }
      }),
    ];
    return () => {
      pending.forEach((promise) => void promise.then((unlisten) => unlisten()));
    };
  }, [queryClient]);

  const remove = useMutation({
    mutationFn: (runtime: ManagedRuntime) => ipc.managedRuntimeDelete(runtime.featureVersion),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["managed-runtimes"] });
      void queryClient.invalidateQueries({ queryKey: ["managed-runtimes-size"] });
      void queryClient.invalidateQueries({ queryKey: ["java"] });
      toast.success("Runtime removed");
    },
    onError: (error: unknown) => toastError(error),
  });

  const setSystemOnly = useMutation({
    mutationFn: (next: boolean) =>
      ipc.settingsSet("use_system_java_only", next ? "true" : "false"),
    onSuccess: () => void queryClient.invalidateQueries({ queryKey: ["settings"] }),
    onError: (error: unknown) => toastError(error),
  });

  const rows = runtimes.data ?? [];

  return (
    <section className="grid gap-3">
      <header className="flex flex-wrap items-center justify-between gap-2">
        <div>
          <h3 className="flex items-center gap-2 text-sm font-semibold">
            <HardDrive className="size-4" aria-hidden />
            Java downloaded by this app
          </h3>
          <p className="text-xs text-muted-foreground">
            {rows.length === 0
              ? "None yet. One is offered when a server needs a version you do not have."
              : `${rows.length} runtime${rows.length === 1 ? "" : "s"}, ${formatBytes(
                  total.data ?? 0,
                )} on disk`}
          </p>
        </div>
      </header>

      {busy ? (
        <p className="rounded-md border border-border bg-muted/40 px-3 py-2 text-xs" role="status">
          {busy}
        </p>
      ) : null}

      <ul className="grid gap-2">
        {rows.map((runtime) => (
          <li
            key={runtime.featureVersion}
            className="flex flex-wrap items-center justify-between gap-3 rounded-lg border border-border px-3 py-2"
          >
            <div className="min-w-0">
              <p className="text-sm">
                Java {runtime.featureVersion} · {runtime.releaseName}
              </p>
              <p className="truncate text-xs text-muted-foreground" title={runtime.javaPath}>
                {formatBytes(runtime.sizeBytes)} ·{" "}
                {runtime.usedBy.length === 0
                  ? "not used by any server"
                  : `used by ${runtime.usedBy.join(", ")}`}
              </p>
            </div>
            <Button
              size="sm"
              variant="ghost"
              aria-label={`Remove the downloaded Java ${runtime.featureVersion}`}
              // Kept clickable when in use: the refusal names the servers, which
              // is more useful than a disabled button that explains nothing.
              onClick={() => remove.mutate(runtime)}
            >
              <Trash2 />
            </Button>
          </li>
        ))}
      </ul>

      <label className="flex items-start justify-between gap-3 text-sm">
        <span>
          Use only the Java installed on this computer
          <span className="block text-xs text-muted-foreground">
            Nothing is downloaded. A server whose version is missing refuses to start instead.
          </span>
        </span>
        <Switch
          checked={systemOnly}
          onCheckedChange={(next) => setSystemOnly.mutate(next)}
          aria-label="Use only the Java installed on this computer"
        />
      </label>
    </section>
  );
}

/**
 * The inline offer: what this server needs, and the download that provides it.
 *
 * Shown where an instance is set up rather than at launch, because "install
 * Java 25 first" is something to learn while choosing a version, not after
 * pressing Start.
 */
export function JavaPlanNotice({
  mcVersion,
  recordedMajor,
  pinned,
}: {
  mcVersion: string;
  recordedMajor?: number | null;
  pinned?: string | null;
}) {
  const queryClient = useQueryClient();
  const [starting, setStarting] = useState(false);

  const plan = useQuery({
    queryKey: ["java-plan", mcVersion, recordedMajor ?? null, pinned ?? null],
    queryFn: () => ipc.javaPlanFor(mcVersion, recordedMajor ?? null, pinned ?? null),
    enabled: mcVersion.trim().length > 0,
  });

  useEffect(() => {
    const pending = onTaskDone((payload) => {
      if (payload.kind !== "java_download") return;
      setStarting(false);
      void queryClient.invalidateQueries({ queryKey: ["java-plan"] });
    });
    return () => void pending.then((unlisten) => unlisten());
  }, [queryClient]);

  if (!plan.data) return null;
  const { requiredMajor, satisfied, origin, offer, offerError } = plan.data;

  if (satisfied) {
    return (
      <p className="text-xs text-muted-foreground">
        Needs Java {requiredMajor} —{" "}
        {origin === "managed"
          ? "using the copy this app downloaded"
          : origin === "pinned"
            ? "using the runtime pinned for this server"
            : "found on this computer"}
        .
      </p>
    );
  }

  return (
    <div className="rounded-md border border-[var(--status-starting)]/40 bg-[var(--status-starting)]/10 p-3 text-xs">
      <p className="font-medium">
        This version needs Java {requiredMajor}, which is not installed.
      </p>
      {offer ? (
        <>
          <p className="mt-1 text-muted-foreground">
            {offer.releaseName} for {offer.os}/{offer.arch}, {formatBytes(offer.sizeBytes)} to
            download. It is shared with every other server needing Java {requiredMajor}.
          </p>
          <Button
            size="sm"
            className="mt-2"
            disabled={starting}
            onClick={() => {
              setStarting(true);
              ipc
                .managedRuntimeInstall(requiredMajor)
                .catch((error: unknown) => {
                  setStarting(false);
                  toastError(error);
                });
            }}
          >
            <Download /> {starting ? "Downloading…" : `Download Java ${requiredMajor}`}
          </Button>
        </>
      ) : (
        <p className="mt-1 text-muted-foreground">{offerError}</p>
      )}
    </div>
  );
}
