import { useQuery } from "@tanstack/react-query";
import {
  AlertTriangle,
  ExternalLink,
  Info,
  Loader2,
  Map as MapIcon,
  RefreshCw,
} from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/misc";
import { copyToClipboard } from "@/lib/clipboard";
import { openExternal } from "@/lib/external";
import { ipc } from "@/lib/ipc";
import { toastError } from "@/lib/toast";
import type { InstanceView } from "@/lib/types";

/**
 * The server's own web map, embedded.
 *
 * squaremap runs a small web server inside the Minecraft server, so there is
 * nothing to render here — the work is pointing at the right port and being
 * honest about the states where there is nothing to show: the server is
 * stopped, the config has not been written yet, or nothing has been rendered.
 */
export function MapTab({ instance }: { instance: InstanceView }) {
  // Reloading an iframe means changing its key: the page inside is not ours.
  const [reloads, setReloads] = useState(0);
  const [rendering, setRendering] = useState(false);
  const [moving, setMoving] = useState(false);

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
          squaremap
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
        <div className="rounded-md border border-[var(--status-starting)]/40 bg-[var(--status-starting)]/10 px-3 py-2 text-xs">
          <p className="flex items-start gap-2">
            <AlertTriangle
              className="mt-0.5 size-3.5 shrink-0 text-[var(--status-starting)]"
              aria-hidden
            />
            <span>
              Port {map.port} is also used by {map.conflict}. Whichever server starts second will
              have no map.
            </span>
          </p>
          <div className="mt-2 flex flex-wrap items-center gap-2">
            <Button
              size="sm"
              variant="outline"
              disabled={map.running || moving}
              onClick={() => {
                setMoving(true);
                ipc
                  .mapMovePort(instance.id)
                  .then((port) => {
                    void status.refetch();
                    toast.success(port ? `The map will use port ${port}` : "Nothing to change", {
                      description: port
                        ? "Takes effect the next time this server starts."
                        : undefined,
                    });
                  })
                  .catch((error: unknown) => toastError(error))
                  .finally(() => setMoving(false));
              }}
            >
              {moving ? "Moving…" : "Move to a free port"}
            </Button>
            <span className="text-muted-foreground">
              {map.running
                ? "Stop the server first: a running server rewrites the file on shutdown."
                : `Edits ${map.configPath ?? "the map's config"}.`}
            </span>
          </div>
        </div>
      ) : null}

      {map.barelyRendered && map.port ? (
        // A map with nothing rendered is a blank rectangle, which reads as
        // broken rather than as empty. squaremap draws chunks as they are
        // played, so a new world is *supposed* to look like this.
        <div className="rounded-md border border-border px-3 py-2 text-xs">
          <p className="flex items-start gap-2">
            <Info className="mt-0.5 size-3.5 shrink-0 text-muted-foreground" aria-hidden />
            <span>
              squaremap renders areas as they are explored and saved. A new world will look
              mostly empty until you have played in it.
            </span>
          </p>
          {map.renderCommand ? (
            <div className="mt-2 flex flex-wrap items-center gap-2">
              <Button
                size="sm"
                variant="outline"
                disabled={!map.running || rendering}
                onClick={() => {
                  setRendering(true);
                  ipc
                    .mapRenderWorld(instance.id)
                    .then((command) =>
                      toast.success("Rendering started", {
                        description: `Sent ${command}. Watch the Console tab for progress.`,
                      }),
                    )
                    .catch((error: unknown) => toastError(error))
                    .finally(() => setRendering(false));
                }}
              >
                {rendering ? "Sending…" : "Render existing world now"}
              </Button>
              <span className="text-muted-foreground">
                {map.running
                  ? `Sends ${map.renderCommand}, which draws what has already been played.`
                  : "The server has to be running to render."}
              </span>
            </div>
          ) : null}
        </div>
      ) : null}

      {map.reachesTheNetwork && map.port ? (
        // squaremap binds 0.0.0.0 by default, so this is not a private page on
        // this computer — saying so here is the difference between a choice and
        // a surprise.
        <p className="flex items-start gap-2 rounded-md border border-border px-3 py-2 text-xs text-muted-foreground">
          <Info className="mt-0.5 size-3.5 shrink-0" aria-hidden />
          <span>
            This map listens on {map.bind ?? "0.0.0.0"}, so anyone who can reach this computer on
            port {map.port} can see it. The Networking tab lists the addresses that reach it.
          </span>
        </p>
      ) : null}

      {!map.port ? (
        <Placeholder
          title="No map address yet"
          detail="squaremap writes its configuration the first time the server starts. Start the server once, then come back."
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
