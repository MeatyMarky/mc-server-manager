import { openUrl } from "@tauri-apps/plugin-opener";
import { ExternalLink, ShieldCheck } from "lucide-react";
import { useId } from "react";

import { Button } from "@/components/ui/button";
import type { InstanceView } from "@/lib/types";
import { useAcceptEula, useEula } from "./queries";

/**
 * The EULA gate, per instance.
 *
 * A checkbox rather than a toggle, because this is a legal agreement and the
 * two states have to be unmistakable. It starts unchecked, reflects only what
 * is really recorded, and nothing in the backend writes `eula=true` without a
 * click here. The exact file being written is named on screen.
 */
export function EulaCard({ instance }: { instance: InstanceView }) {
  const eula = useEula(instance.id);
  const accept = useAcceptEula(instance.id);
  const checkboxId = useId();
  const describedBy = useId();

  if (eula.isLoading || !eula.data) return null;

  const status = eula.data;

  return (
    <section
      className="rounded-lg border border-border p-4"
      aria-labelledby={`${checkboxId}-heading`}
    >
      <h3
        id={`${checkboxId}-heading`}
        className="flex items-center gap-2 text-sm font-semibold"
      >
        <ShieldCheck className="size-4" aria-hidden />
        Minecraft EULA
      </h3>

      <p id={describedBy} className="mt-1 text-sm text-muted-foreground">
        A Minecraft server refuses to start until Mojang's EULA is accepted for it. Ticking this
        box writes <code>eula=true</code> into{" "}
        <span className="font-mono text-xs">{status.path}</span>. Nothing else in this app writes
        it, and unticking rewrites the same file to <code>eula=false</code>.
      </p>

      <div className="mt-3 flex flex-wrap items-center gap-3">
        <label className="flex items-start gap-2 text-sm">
          <input
            id={checkboxId}
            type="checkbox"
            className="mt-0.5 size-4 shrink-0 accent-primary focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring"
            checked={status.accepted}
            disabled={accept.isPending}
            aria-describedby={describedBy}
            onChange={(event) => accept.mutate(event.target.checked)}
          />
          <span>
            I have read and accept the Minecraft End User Licence Agreement for{" "}
            <strong>{instance.name}</strong>.
          </span>
        </label>

        <Button variant="outline" size="sm" onClick={() => void openUrl(status.url)}>
          <ExternalLink /> Read the EULA
        </Button>
      </div>

      <p className="mt-2 text-xs text-muted-foreground" aria-live="polite">
        {status.accepted && status.acceptedAt
          ? `Accepted on this computer at ${new Date(status.acceptedAt).toLocaleString()}.`
          : status.fileExists
            ? "Not accepted. The server will refuse to start."
            : "Not accepted yet. No eula.txt has been written for this server."}
      </p>
    </section>
  );
}
