import { openUrl } from "@tauri-apps/plugin-opener";
import { ExternalLink, ShieldCheck } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/misc";
import { Label } from "@/components/ui/dialog";
import type { InstanceView } from "@/lib/types";
import { useAcceptEula, useEula } from "./queries";

/**
 * The EULA gate. Nothing in the backend writes `eula=true` on its own; this
 * toggle is the only path, and it records who accepted and when.
 */
export function EulaCard({ instance }: { instance: InstanceView }) {
  const eula = useEula(instance.id);
  const accept = useAcceptEula(instance.id);

  if (eula.isLoading || !eula.data) return null;

  const accepted = eula.data.accepted;

  return (
    <div className="rounded-lg border border-border p-4">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="max-w-lg">
          <h3 className="flex items-center gap-2 text-sm font-semibold">
            <ShieldCheck className="size-4" />
            Minecraft EULA
          </h3>
          <p className="mt-1 text-sm text-muted-foreground">
            A Minecraft server refuses to start until you accept Mojang's EULA. Accepting
            here writes <code>eula=true</code> into this instance's <code>eula.txt</code>;
            nothing writes it for you.
          </p>
          {eula.data.acceptedAt ? (
            <p className="mt-1 text-xs text-muted-foreground">
              Accepted {new Date(eula.data.acceptedAt).toLocaleString()}
            </p>
          ) : null}
        </div>

        <div className="flex items-center gap-3">
          <Button
            variant="ghost"
            size="sm"
            onClick={() => void openUrl(eula.data.url)}
          >
            <ExternalLink /> Read it
          </Button>
          <Label htmlFor="eula-toggle" className="sr-only">
            Accept the Minecraft EULA
          </Label>
          <Switch
            id="eula-toggle"
            checked={accepted}
            disabled={accept.isPending}
            onCheckedChange={(next) => accept.mutate(next)}
          />
        </div>
      </div>
    </div>
  );
}
