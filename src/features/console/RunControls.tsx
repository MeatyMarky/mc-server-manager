import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Play, RotateCw, Square, TriangleAlert } from "lucide-react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { instanceKeys } from "@/features/instances/queries";
import { ipc } from "@/lib/ipc";
import { toastError } from "@/lib/toast";
import type { InstanceView, StopStage } from "@/lib/types";

const STOP_MESSAGE: Record<StopStage, string> = {
  already_stopped: "It was not running",
  graceful: "Stopped cleanly",
  terminated: "It ignored the stop command and was terminated",
  killed: "It had to be killed",
};

/** Start / stop / restart, plus the port conflict warning shown before a start. */
export function RunControls({
  instance,
  onStartError,
}: {
  instance: InstanceView;
  /// Called with whatever stopped the server from starting, so the Console tab
  /// can keep it on screen; a toast for "no Java found" is gone before the user
  /// has read the fix.
  onStartError?: (error: unknown) => void;
}) {
  const queryClient = useQueryClient();
  const stopped = instance.status === "stopped" || instance.status === "crashed";
  const live =
    instance.status === "running" ||
    instance.status === "starting" ||
    instance.status === "stopping" ||
    instance.status === "unmanaged";

  // Only meaningful while stopped, and cheap: a socket probe plus one query.
  const port = useQuery({
    queryKey: ["port-status", instance.id],
    queryFn: () => ipc.portStatus(instance.id),
    enabled: stopped && instance.status !== "missing",
    refetchOnMount: "always",
  });

  const invalidate = () => {
    void queryClient.invalidateQueries({ queryKey: instanceKeys.all });
    void queryClient.invalidateQueries({ queryKey: ["port-status", instance.id] });
  };

  const start = useMutation({
    mutationFn: () => ipc.instanceStart(instance.id),
    onSuccess: () => {
      invalidate();
      onStartError?.(null);
      toast.success(`Starting "${instance.name}"`);
    },
    onError: (error: unknown) => {
      onStartError?.(error);
      toastError(error);
    },
  });

  const stop = useMutation({
    mutationFn: () => ipc.instanceStop(instance.id),
    onSuccess: (stage) => {
      invalidate();
      const description = STOP_MESSAGE[stage];
      if (stage === "killed" || stage === "terminated") {
        toast.warning(`"${instance.name}" stopped`, { description });
      } else {
        toast.success(`"${instance.name}" stopped`, { description });
      }
    },
    onError: (error: unknown) => toastError(error),
  });

  const restart = useMutation({
    mutationFn: () => ipc.instanceRestart(instance.id),
    onSuccess: () => {
      invalidate();
      toast.success(`Restarting "${instance.name}"`);
    },
    onError: (error: unknown) => toastError(error),
  });

  const busy = start.isPending || stop.isPending || restart.isPending;

  if (instance.status === "missing") return null;

  return (
    <div className="flex items-center gap-2">
      {port.data && stopped ? (
        <span
          className="flex items-center gap-1 text-xs text-destructive"
          title={port.data}
          role="status"
        >
          <TriangleAlert className="size-3.5" />
          port conflict
        </span>
      ) : null}

      {live ? (
        <>
          {instance.status === "unmanaged" ? null : (
            <Button
              variant="outline"
              size="sm"
              disabled={busy}
              onClick={() => restart.mutate()}
            >
              <RotateCw /> Restart
            </Button>
          )}
          <Button
            variant="destructive"
            size="sm"
            disabled={busy}
            onClick={() => stop.mutate()}
          >
            <Square /> {stop.isPending ? "Stopping…" : "Stop"}
          </Button>
        </>
      ) : (
        <Button
          size="sm"
          disabled={busy || !instance.installedAt || !instance.eulaAccepted}
          title={
            !instance.installedAt
              ? "Install the server first"
              : !instance.eulaAccepted
                ? "Accept the EULA first"
                : port.data ?? undefined
          }
          onClick={() => start.mutate()}
        >
          <Play /> {start.isPending ? "Starting…" : "Start"}
        </Button>
      )}
    </div>
  );
}
