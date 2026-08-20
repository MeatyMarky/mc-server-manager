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
 * BlueMap and Dynmap each run a small web server inside the Minecraft server,
 * so there is nothing to render here — the work is pointing at the right port
 * and being honest about the two states where there is nothing to show: the
 * server is stopped, or the mod has not written its config yet.
 */
export function MapTab({ instance }: { instance: InstanceView }) {
  // Reloading an iframe means changing its key: the page inside is not ours.
  const [reloads, setReloads] = useState(0);
  const [allowing, setAllowing] = useState(false);
  const [rendering, setRendering] = useState(false);

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
  const mapName = map.kind === "blue_map" ? "BlueMap" : "Dynmap";

  return (
    <div className="flex h-full min-h-0 flex-col gap-3">
      <div className="flex flex-wrap items-center gap-2">
        <Badge>
          <MapIcon className="mr-1 inline size-3.5" aria-hidden />
          {mapName}
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

      {map.downloadBlocked ? (
        <div className="rounded-md border border-[var(--status-starting)]/40 bg-[var(--status-starting)]/10 px-3 py-2 text-xs">
          <p className="flex items-start gap-2">
            <AlertTriangle
              className="mt-0.5 size-3.5 shrink-0 text-[var(--status-starting)]"
              aria-hidden
            />
            <span>
              BlueMap will not render until it is allowed to download a Minecraft client jar
              from Mojang, which is where it gets block textures. Its config currently says no,
              and the server stops on start with "BlueMap is missing important resources!".
              Allowing it says you own Minecraft: Java Edition and accept Mojang's EULA.
            </span>
          </p>
          <Button
            size="sm"
            className="mt-2"
            disabled={allowing}
            onClick={() => {
              setAllowing(true);
              ipc
                .mapAcceptDownload(instance.id)
                .then(() => status.refetch())
                .catch((error: unknown) => toastError(error))
                .finally(() => setAllowing(false));
            }}
          >
            {allowing ? "Saving…" : "Allow the download"}
          </Button>
        </div>
      ) : null}

      {map.barelyRendered && map.port ? (
        // A map with nothing rendered is a black rectangle, which reads as
        // broken rather than as empty. Both maps draw chunks as they are
        // played, so a new world is *supposed* to look like this.
        <div className="rounded-md border border-border px-3 py-2 text-xs">
          <p className="flex items-start gap-2">
            <Info className="mt-0.5 size-3.5 shrink-0 text-muted-foreground" aria-hidden />
            <span>
              {mapName} renders areas as they are explored and saved. A new world will look
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
          detail={`${mapName} writes its configuration the first time the server starts. Start the server once, then come back.`}
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
