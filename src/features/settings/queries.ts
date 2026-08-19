import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { ipc } from "@/lib/ipc";
import { toastError } from "@/lib/toast";

/**
 * The app-wide settings row set, as one query.
 *
 * Everything in the `settings` table is a string, and every panel that reads
 * one reads the same snapshot, so a change made in one section is visible to
 * the others without a reload.
 */
export function useSettings() {
  return useQuery({ queryKey: ["settings"], queryFn: () => ipc.settingsGetAll() });
}

/** Write one setting and refresh whatever reads it. */
export function useSetSetting(...alsoInvalidate: string[]) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ key, value }: { key: string; value: string }) => ipc.settingsSet(key, value),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["settings"] });
      void queryClient.invalidateQueries({ queryKey: ["app-info"] });
      for (const key of alsoInvalidate) {
        void queryClient.invalidateQueries({ queryKey: [key], exact: false });
      }
    },
    onError: (error: unknown) => toastError(error),
  });
}
