// Backend-derived state. Mutations invalidate; the backend also emits
// `instances://changed`, which useInstanceEvents() turns into an invalidation.
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect } from "react";
import { toast } from "sonner";

import { ipc } from "@/lib/ipc";
import { toastError } from "@/lib/toast";
import { onInstanceStatus, onInstancesChanged } from "@/lib/events";
import type {
  CloneInstanceInput,
  CreateInstanceInput,
  ImportInstanceInput,
  UpdateInstanceInput,
} from "@/lib/types";

export const instanceKeys = {
  all: ["instances"] as const,
  detail: (id: number) => ["instances", id] as const,
  appInfo: ["app-info"] as const,
};

export function useInstances() {
  return useQuery({
    queryKey: instanceKeys.all,
    queryFn: ipc.instanceList,
  });
}

export function useAppInfo() {
  return useQuery({
    queryKey: instanceKeys.appInfo,
    queryFn: ipc.appInfo,
    staleTime: Infinity,
  });
}

/** Subscribes to backend events once, for the lifetime of the app shell. */
export function useInstanceEvents() {
  const queryClient = useQueryClient();

  useEffect(() => {
    const unlisteners = [
      onInstancesChanged(() => {
        void queryClient.invalidateQueries({ queryKey: instanceKeys.all });
      }),
      onInstanceStatus(() => {
        void queryClient.invalidateQueries({ queryKey: instanceKeys.all });
      }),
    ];
    return () => {
      unlisteners.forEach((pending) => {
        void pending.then((unlisten) => unlisten());
      });
    };
  }, [queryClient]);
}

function useInstanceMutation<TInput, TResult>(
  mutationFn: (input: TInput) => Promise<TResult>,
  successMessage: (result: TResult, input: TInput) => string,
) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn,
    onSuccess: (result, input) => {
      void queryClient.invalidateQueries({ queryKey: instanceKeys.all });
      toast.success(successMessage(result, input));
    },
    onError: (error: unknown) => toastError(error),
  });
}

export function useCreateInstance() {
  return useInstanceMutation(
    (input: CreateInstanceInput) => ipc.instanceCreate(input),
    (created) => `Created "${created.name}"`,
  );
}

export function useCloneInstance() {
  return useInstanceMutation(
    (input: CloneInstanceInput) => ipc.instanceClone(input),
    (created) => `Cloned to "${created.name}"`,
  );
}

export function useImportInstance() {
  return useInstanceMutation(
    (input: ImportInstanceInput) => ipc.instanceImport(input),
    (imported) => `Imported "${imported.name}"`,
  );
}

export function useRenameInstance() {
  return useInstanceMutation(
    ({ id, name }: { id: number; name: string }) => ipc.instanceRename(id, name),
    (renamed) => `Renamed to "${renamed.name}"`,
  );
}

export function useUpdateInstance() {
  return useInstanceMutation(
    ({ id, input }: { id: number; input: UpdateInstanceInput }) =>
      ipc.instanceUpdate(id, input),
    () => "Settings saved",
  );
}

export function useDeleteInstance() {
  return useInstanceMutation(
    ({ id, deleteFiles }: { id: number; deleteFiles: boolean }) =>
      ipc.instanceDelete(id, deleteFiles),
    (report) =>
      report.filesDeleted
        ? `Deleted "${report.name}" and its files`
        : `Removed "${report.name}" from the list; files kept`,
  );
}

export function useLocateInstance() {
  return useInstanceMutation(
    ({ id, path }: { id: number; path: string }) => ipc.instanceLocate(id, path),
    (located) => `"${located.name}" now points at ${located.path}`,
  );
}

export function useForceStopInstance() {
  return useInstanceMutation(
    (id: number) => ipc.instanceForceStop(id),
    (stopped) => `Force stopped "${stopped.name}"`,
  );
}
