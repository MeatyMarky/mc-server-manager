import { useQuery } from "@tanstack/react-query";
import { AlertTriangle, ExternalLink, Loader2, Map as MapIcon, RefreshCw } from "lucide-react";
import { useState } from "react";

import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/misc";
import { copyToClipboard } from "@/lib/clipboard";
import { openExternal } from "@/lib/external";
import { ipc } from "@/lib/ipc";
import type { InstanceView } from "@/lib/types";

/**
 * The server's own web map, embedded.
 *
 * BlueMap and Dynmap each run a small web server inside the Minecraft server,
 * so there is nothing to render here — the work is pointing at the right port
 * and being honest about the two states where there is nothing to show: the
 * server is stopped, or the mod has not written its config yet.
 */
export function MapTab({ instance }: { instance: InstanceView }) {
  // Reloading an iframe means changing its key: the page inside is not ours.
  const [reloads, setReloads] = useState(0);

  const status = useQuery({
    queryKey: ["map", instance.id, instance.status],
    queryFn: () => ipc.mapStatus(instance.id),
  });

  if (status.isLoading || !status.data) {
    return (
      <p className="flex items-center gap-2 p-6 text-sm text-muted-foreground">
        <Loader2 className="size-4 animate-spin" aria-hidden />
        Looking for the map…
      </p>
    );
  }

  const map = status.data;

  return (
    <div className="flex h-full min-h-0 flex-col gap-3">
      <div className="flex flex-wrap items-center gap-2">
        <Badge>
          <MapIcon className="mr-1 inline size-3.5" aria-hidden />
          {map.kind === "blue_map" ? "BlueMap" : "Dynmap"}
        </Badge>
        {map.port ? (
          <>
            <code className="rounded bg-muted px-2 py-1 font-mono text-xs">
              127.0.0.1:{map.port}
            </code>
            <Button
              variant="ghost"
              size="sm"
              onClick={() => void copyToClipboard(`127.0.0.1:${map.port}`)}
            >
              Copy
            </Button>
          </>
        ) : null}

        <div className="ml-auto flex items-center gap-2">
          {map.url && map.running ? (
            <Button variant="ghost" size="sm" onClick={() => setReloads((count) => count + 1)}>
              <RefreshCw /> Reload
            </Button>
          ) : null}
          {map.url ? (
            // A real browser handles a big map better than an embedded frame,
            // and it is the only way to keep the map open while working here.
            <Button variant="outline" size="sm" onClick={() => void openExternal(map.url!)}>
              <ExternalLink /> Open in browser
            </Button>
          ) : null}
        </div>
      </div>

      {map.conflict ? (
        <p className="flex items-start gap-2 rounded-md border border-[var(--status-starting)]/40 bg-[var(--status-starting)]/10 px-3 py-2 text-xs">
          <AlertTriangle
            className="mt-0.5 size-3.5 shrink-0 text-[var(--status-starting)]"
            aria-hidden
          />
          <span>
            Port {map.port} is also used by {map.conflict}. Whichever server starts second will
            have no map — change it in {map.configPath ?? "the map's config"}.
          </span>
        </p>
      ) : null}

      {!map.port ? (
        <Placeholder
          title="No map address yet"
          detail={`${
            map.kind === "blue_map" ? "BlueMap" : "Dynmap"
          } writes its configuration the first time the server starts. Start the server once, then come back.`}
        />
      ) : !map.running ? (
        <Placeholder
          title="The server is not running"
          detail="The map is served by the server itself, so it answers only while the server is up. Start it from the Console tab."
        />
      ) : (
        <iframe
          key={reloads}
          src={map.url ?? undefined}
          title={`${instance.name} map`}
          className="min-h-0 flex-1 rounded-md border border-border bg-card"
          // The map is a local page from a mod, not part of this app: it gets
          // no access to what is around it.
          sandbox="allow-scripts allow-same-origin"
          referrerPolicy="no-referrer"
        />
      )}
    </div>
  );
}

function Placeholder({ title, detail }: { title: string; detail: string }) {
  return (
    <div className="flex min-h-0 flex-1 items-center justify-center rounded-md border border-dashed border-border p-8">
      <div className="max-w-md text-center">
        <MapIcon className="mx-auto size-8 text-muted-foreground" aria-hidden />
        <h3 className="mt-3 text-sm font-medium">{title}</h3>
        <p className="mt-1 text-sm text-muted-foreground">{detail}</p>
      </div>
    </div>
  );
}
