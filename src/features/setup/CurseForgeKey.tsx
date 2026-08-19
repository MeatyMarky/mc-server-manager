import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { CheckCircle2, ExternalLink, KeyRound } from "lucide-react";
import { useEffect, useState } from "react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { ipc } from "@/lib/ipc";
import { openExternal } from "@/lib/external";
import { toastError } from "@/lib/toast";

const SETTING = "curseforge_api_key";

/**
 * The CurseForge key.
 *
 * CurseForge requires every application to use its own key and forbids shipping
 * one inside a distributed app, so this is a thing the user has to do — which
 * means the box has to say why, rather than presenting an unexplained field.
 */
export function CurseForgeKey() {
  const queryClient = useQueryClient();
  const [draft, setDraft] = useState("");

  const settings = useQuery({ queryKey: ["settings"], queryFn: () => ipc.settingsGetAll() });
  const sources = useQuery({ queryKey: ["mod-sources"], queryFn: () => ipc.modsSources() });

  const stored = settings.data?.[SETTING] ?? "";
  useEffect(() => setDraft(stored), [stored]);

  const curseforge = sources.data?.find((entry) => entry.id === "curse_forge");

  const save = useMutation({
    mutationFn: (value: string) => ipc.settingsSet(SETTING, value.trim()),
    onSuccess: (_result, value) => {
      void queryClient.invalidateQueries({ queryKey: ["settings"] });
      void queryClient.invalidateQueries({ queryKey: ["mod-sources"] });
      void queryClient.invalidateQueries({ queryKey: ["mod-search"] });
      toast.success(value.trim() === "" ? "CurseForge key removed" : "CurseForge key saved");
    },
    onError: (error: unknown) => toastError(error),
  });

  return (
    <section className="grid gap-2">
      <h3 className="flex items-center gap-2 text-sm font-semibold">
        <KeyRound className="size-4" aria-hidden />
        CurseForge
      </h3>

      <p className="text-xs text-muted-foreground">
        {curseforge?.configured ? (
          <span className="flex items-center gap-1.5">
            <CheckCircle2 className="size-3.5 text-emerald-500" aria-hidden />
            Configured — CurseForge is available in the browser.
          </span>
        ) : (
          (curseforge?.needs ??
            "CurseForge requires every application to use its own API key, so this app cannot ship one.")
        )}
      </p>

      <div className="flex flex-wrap items-end gap-2">
        <div className="grid min-w-64 flex-1 gap-1.5">
          <Label htmlFor="curseforge-key">API key</Label>
          <Input
            id="curseforge-key"
            // A key is a secret, so it is not left legible on screen.
            type="password"
            autoComplete="off"
            spellCheck={false}
            placeholder="$2a$10$…"
            value={draft}
            onChange={(event) => setDraft(event.target.value)}
          />
        </div>
        <Button size="sm" disabled={save.isPending || draft === stored} onClick={() => save.mutate(draft)}>
          Save
        </Button>
        {stored ? (
          <Button
            size="sm"
            variant="ghost"
            disabled={save.isPending}
            onClick={() => save.mutate("")}
          >
            Remove
          </Button>
        ) : null}
        {curseforge?.setupUrl ? (
          <Button size="sm" variant="outline" onClick={() => void openExternal(curseforge.setupUrl!)}>
            <ExternalLink /> Get a key
          </Button>
        ) : null}
      </div>

      <p className="text-xs text-muted-foreground">
        The key is stored in this app's database on this computer and sent only to CurseForge.
      </p>
    </section>
  );
}
