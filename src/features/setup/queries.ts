// Backend state for installs, the EULA and Java. Install progress arrives as
// events, so this module owns the subscription and exposes it as a hook.
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { toast } from "sonner";

import { instanceKeys } from "@/features/instances/queries";
import { onTaskDone, onTaskProgress } from "@/lib/events";
import { ipc } from "@/lib/ipc";
import { toastError } from "@/lib/toast";
import type { ServerType, TaskDoneEvent } from "@/lib/types";

export const setupKeys = {
  versions: (serverType: ServerType) => ["versions", serverType] as const,
  builds: (serverType: ServerType, mcVersion: string) =>
    ["builds", serverType, mcVersion] as const,
  eula: (id: number) => ["eula", id] as const,
  java: ["java"] as const,
  javaStatus: (id: number) => ["java-status", id] as const,
};

export function useProviderVersions(serverType: ServerType, enabled = true) {
  return useQuery({
    queryKey: setupKeys.versions(serverType),
    queryFn: () => ipc.providerVersions(serverType),
    enabled,
    staleTime: 10 * 60 * 1000,
  });
}

export function useProviderBuilds(serverType: ServerType, mcVersion: string, enabled = true) {
  return useQuery({
    queryKey: setupKeys.builds(serverType, mcVersion),
    queryFn: () => ipc.providerBuilds(serverType, mcVersion),
    enabled: enabled && Boolean(mcVersion),
    staleTime: 10 * 60 * 1000,
  });
}

export function useEula(id: number) {
  return useQuery({
    queryKey: setupKeys.eula(id),
    queryFn: () => ipc.eulaGet(id),
  });
}

export function useAcceptEula(id: number) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (accepted: boolean) => ipc.eulaSet(id, accepted),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: setupKeys.eula(id) });
      void queryClient.invalidateQueries({ queryKey: instanceKeys.all });
    },
    onError: (error: unknown) => toastError(error),
  });
}

export function useJavaRuntimes() {
  return useQuery({ queryKey: setupKeys.java, queryFn: ipc.javaList });
}

export function useJavaStatus(id: number) {
  return useQuery({
    queryKey: setupKeys.javaStatus(id),
    queryFn: () => ipc.javaStatus(id),
  });
}

export function useRescanJava() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ipc.javaRescan,
    onSuccess: (runtimes) => {
      void queryClient.invalidateQueries({ queryKey: setupKeys.java });
      void queryClient.invalidateQueries({ queryKey: ["java-status"] });
      toast.success(
        runtimes.length === 1
          ? "Found 1 Java runtime"
          : `Found ${runtimes.length} Java runtimes`,
      );
    },
    onError: (error: unknown) => toastError(error),
  });
}

export function useAddJava() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (path: string) => ipc.javaAddManual(path),
    onSuccess: (runtime) => {
      void queryClient.invalidateQueries({ queryKey: setupKeys.java });
      void queryClient.invalidateQueries({ queryKey: ["java-status"] });
      toast.success(`Added Java ${runtime.major} from ${runtime.path}`);
    },
    onError: (error: unknown) => toastError(error),
  });
}

export interface InstallProgress {
  taskId: string;
  phase: string;
  done: number;
  total: number | null;
  message: string;
}

/**
 * Tracks one instance's install: starts it, follows `task://progress`, and
 * surfaces the failure (including an installer log) rather than a toast alone.
 */
export function useInstall(instanceId: number) {
  const queryClient = useQueryClient();
  const [progress, setProgress] = useState<InstallProgress | null>(null);
  const [failure, setFailure] = useState<TaskDoneEvent | null>(null);
  const [taskId, setTaskId] = useState<string | null>(null);

  useEffect(() => {
    const unlisteners = [
      onTaskProgress((payload) => {
        if (payload.instanceId !== instanceId) return;
        setProgress({
          taskId: payload.taskId,
          phase: payload.phase,
          done: payload.done,
          total: payload.total,
          message: payload.message,
        });
      }),
      onTaskDone((payload) => {
        if (payload.instanceId !== instanceId) return;
        setProgress(null);
        setTaskId(null);
        void queryClient.invalidateQueries({ queryKey: instanceKeys.all });
        void queryClient.invalidateQueries({ queryKey: setupKeys.eula(instanceId) });
        void queryClient.invalidateQueries({ queryKey: setupKeys.javaStatus(instanceId) });

        if (payload.ok) {
          toast.success("Server installed");
        } else if (payload.cancelled) {
          toast.message("Install cancelled", {
            description: "The partial download is kept, so a retry resumes it.",
          });
        } else if (payload.logPath) {
          // Installer failures get their own state with the log, not a toast.
          setFailure(payload);
        } else {
          toast.error(payload.error ?? "The install failed");
        }
      }),
    ];
    return () => {
      unlisteners.forEach((pending) => void pending.then((unlisten) => unlisten()));
    };
  }, [instanceId, queryClient]);

  async function start(mcVersion: string, build: string | null) {
    try {
      const id = await ipc.installServer(instanceId, mcVersion, build);
      setTaskId(id);
      setProgress({
        taskId: id,
        phase: "resolve",
        done: 0,
        total: null,
        message: "Starting…",
      });
    } catch (error) {
      toastError(error);
    }
  }

  async function cancel() {
    if (!taskId) return;
    const cancelled = await ipc.taskCancel(taskId);
    if (!cancelled) toast.message("That install already finished");
  }

  return {
    progress,
    failure,
    dismissFailure: () => setFailure(null),
    isInstalling: progress !== null,
    start,
    cancel,
  };
}
