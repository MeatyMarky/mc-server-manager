import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  AlertTriangle,
  CheckCircle2,
  Copy,
  Eye,
  EyeOff,
  Globe,
  HelpCircle,
  Loader2,
  Map as MapIcon,
  Network,
  Router,
  ShieldCheck,
} from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/misc";
import { copyToClipboard } from "@/lib/clipboard";
import { ipc } from "@/lib/ipc";
import { toastError } from "@/lib/toast";
import type { InstanceView, NetAddress, Reachability } from "@/lib/types";

/**
 * How somebody else joins this server.
 *
 * The single most asked question about running a server, and the answer is
 * genuinely complicated: there are several addresses, they work for different
 * people, and whether the internet can reach the port is not knowable from
 * here. Everything on this screen either states a fact or says plainly that it
 * could not find one out.
 */
export function NetworkingTab({ instance }: { instance: InstanceView }) {
  const queryClient = useQueryClient();
  const [publicShown, setPublicShown] = useState(false);
  const [external, setExternal] = useState<Reachability | null>(null);

  const view = useQuery({
    queryKey: ["network", instance.id],
    queryFn: () => ipc.networkView(instance.id),
  });

  const publicIp = useQuery({
    queryKey: ["network-public-ip"],
    queryFn: () => ipc.networkPublicIp(instance.id),
    // Asked of the router, and only once the user has said they want to see it.
    enabled: publicShown,
    staleTime: 5 * 60 * 1000,
  });

  const upnp = useQuery({
    queryKey: ["network-upnp"],
    queryFn: () => ipc.networkUpnpAvailable(),
    staleTime: 5 * 60 * 1000,
  });

  const check = useMutation({
    mutationFn: (host: string) => ipc.networkExternalCheck(instance.id, host),
    onSuccess: (result) => setExternal(result),
    onError: (error: unknown) => toastError(error),
  });

  const map = useMutation({
    mutationFn: (localIp: string) => ipc.networkUpnpMap(instance.id, localIp),
    onSuccess: (result) => {
      if (result.ok) toast.success(result.detail);
      else toast.error(result.detail);
      void queryClient.invalidateQueries({ queryKey: ["network", instance.id] });
    },
    onError: (error: unknown) => toastError(error),
  });

  // Turning the whitelist on is a real change to server.properties, so it is a
  // button rather than advice — but only while the server is stopped: a running
  // server rewrites that file from memory on shutdown and would undo it.
  const enableWhitelist = useMutation({
    mutationFn: () => ipc.propertiesWrite(instance.id, { changes: { "white-list": "true" } }),
    onSuccess: () => {
      toast.success("The whitelist is on", {
        description: "Add the players who should be allowed in on the Players tab.",
      });
      void queryClient.invalidateQueries({ queryKey: ["network", instance.id] });
      void queryClient.invalidateQueries({ queryKey: ["properties", instance.id] });
    },
    onError: (error: unknown) => toastError(error),
  });

  const unmap = useMutation({
    mutationFn: () => ipc.networkUpnpUnmap(instance.id),
    onSuccess: (result) => {
      if (result.ok) toast.success(result.detail);
      else toast.error(result.detail);
    },
    onError: (error: unknown) => toastError(error),
  });

  if (view.isLoading || !view.data) {
    return (
      <p className="flex items-center gap-2 p-6 text-sm text-muted-foreground">
        <Loader2 className="size-4 animate-spin" aria-hidden />
        Looking at this computer's network…
      </p>
    );
  }

  const data = view.data;
  const lan = data.addresses.find((entry) => entry.kind === "lan");
  const stopped = instance.status === "stopped" || instance.status === "crashed";

  return (
    // The tab panel is already the scroll container; a second one here would
    // put a scrollbar inside a scrollbar.
    <div className="pb-8">
      <div className="grid max-w-3xl gap-8">
        <section className="grid gap-3">
          <header>
            <h3 className="flex items-center gap-2 text-sm font-semibold">
              <Network className="size-4" aria-hidden />
              Addresses to share
            </h3>
            <p className="text-xs text-muted-foreground">
              This server listens on port {data.port}. Each address below works for a different
              set of people.
            </p>
          </header>

          {data.addresses.length === 0 ? (
            <p className="rounded-md border border-border px-3 py-2 text-sm text-muted-foreground">
              No network adapters are up, so nobody can reach this computer right now.
            </p>
          ) : (
            <ul className="grid gap-2">
              {data.addresses.map((entry) => (
                <AddressRow key={`${entry.interface}-${entry.address}`} entry={entry} />
              ))}
            </ul>
          )}

          <div className="rounded-md border border-border p-3">
            <div className="flex flex-wrap items-center justify-between gap-2">
              <div>
                <p className="flex items-center gap-2 text-sm font-medium">
                  <Globe className="size-4" aria-hidden />
                  Public address
                </p>
                <p className="text-xs text-muted-foreground">
                  What the internet sees. Hidden until you ask, because this is the one thing on
                  screen you would not want in a screenshot.
                </p>
              </div>
              <Button
                variant="outline"
                size="sm"
                onClick={() => setPublicShown((current) => !current)}
              >
                {publicShown ? <EyeOff /> : <Eye />}
                {publicShown ? "Hide" : "Show"}
              </Button>
            </div>

            {publicShown ? (
              publicIp.isLoading ? (
                <p className="mt-2 flex items-center gap-2 text-xs text-muted-foreground">
                  <Loader2 className="size-3.5 animate-spin" aria-hidden />
                  Asking the router…
                </p>
              ) : publicIp.data ? (
                <div className="mt-2 flex flex-wrap items-center gap-2">
                  <code className="rounded bg-muted px-2 py-1 font-mono text-xs">
                    {publicIp.data.joinable}
                  </code>
                  <CopyButton value={publicIp.data.joinable} />
                  <Button
                    size="sm"
                    variant="outline"
                    disabled={check.isPending}
                    onClick={() => check.mutate(publicIp.data!.address)}
                  >
                    {check.isPending ? "Checking…" : "Check from outside"}
                  </Button>
                  {publicIp.data.carrierNat ? (
                    // The one case where the manual steps cannot help either:
                    // the router's own address is behind the provider's.
                    <p className="w-full text-xs text-[var(--status-starting)]">
                      This address is itself inside your provider's network (CGNAT), so port
                      forwarding cannot make this server reachable. A VPN address above is the
                      way in, or ask your provider for a public address.
                    </p>
                  ) : null}
                </div>
              ) : (
                <p className="mt-2 text-xs text-muted-foreground">
                  No router answered, so this app cannot tell you the public address. Any
                  "what is my IP" site will show it.
                </p>
              )
            ) : null}

            {external ? <ExternalResult result={external} /> : null}
          </div>
        </section>

        {data.map ? (
          <section className="grid gap-3 border-t border-border pt-6">
            <header>
              <h3 className="flex items-center gap-2 text-sm font-semibold">
                <MapIcon className="size-4" aria-hidden />
                The web map
              </h3>
              <p className="text-xs text-muted-foreground">
                {data.map.label} serves the map on port {data.map.port} — a second port, and a
                second thing to forward. Forwarding the game port alone leaves the map
                unreachable from outside.
              </p>
            </header>

            <ul className="grid gap-2">
              {data.map.addresses.map((entry) => (
                <li
                  key={`map-${entry.interface}-${entry.address}`}
                  className="flex flex-wrap items-center gap-2 rounded-md border border-border px-3 py-2"
                >
                  <Badge>{entry.network ?? KIND_LABEL[entry.kind]}</Badge>
                  <code className="rounded bg-muted px-2 py-1 font-mono text-xs">
                    http://{entry.address}:{data.map!.port}
                  </code>
                  <CopyButton value={`http://${entry.address}:${data.map!.port}`} />
                  <span className="w-full text-xs text-muted-foreground sm:w-auto sm:flex-1">
                    {entry.audience}
                  </span>
                </li>
              ))}
            </ul>

            <p className="text-xs text-muted-foreground">
              {data.map.local === "listening"
                ? "Something is listening on the map's port."
                : "Nothing is listening on the map's port — normal while the server is stopped."}
            </p>
          </section>
        ) : null}

        <section className="grid gap-3 border-t border-border pt-6">
          <header>
            <h3 className="flex items-center gap-2 text-sm font-semibold">
              <Router className="size-4" aria-hidden />
              Letting people in from the internet
            </h3>
            <p className="text-xs text-muted-foreground">
              {data.local === "listening"
                ? "Something is listening on this port on this computer."
                : "Nothing is listening on this port yet — that is normal while the server is stopped."}
            </p>
          </header>

          <div className="rounded-md border border-border p-3">
            <p className="text-sm font-medium">Ask the router to forward the port (UPnP)</p>
            <p className="mt-1 text-xs text-muted-foreground">
              {upnp.isLoading
                ? "Looking for a router that supports it…"
                : upnp.data
                  ? `A router at ${upnp.data} answered. Forwarding lasts 12 hours, and this app never does it on its own.`
                  : "No router answered. Many have UPnP switched off, and some ISP-supplied ones do not offer it — the manual steps below do the same job."}
            </p>
            <div className="mt-2 flex flex-wrap gap-2">
              <Button
                size="sm"
                disabled={!upnp.data || !lan || map.isPending}
                onClick={() => lan && map.mutate(lan.address)}
                title={lan ? undefined : "No LAN address to forward to"}
              >
                {map.isPending ? "Asking the router…" : `Forward port ${data.port}`}
              </Button>
              <Button
                size="sm"
                variant="ghost"
                disabled={!upnp.data || unmap.isPending}
                onClick={() => unmap.mutate()}
              >
                {unmap.isPending ? "Removing…" : "Remove the mapping"}
              </Button>
            </div>
          </div>

          <details className="rounded-md border border-border p-3">
            <summary className="cursor-pointer text-sm font-medium">
              Do it by hand instead
            </summary>
            <ol className="mt-2 grid list-decimal gap-1.5 pl-5 text-xs text-muted-foreground">
              {data.manualSteps.map((step) => (
                <li key={step}>{step}</li>
              ))}
            </ol>
          </details>
        </section>

        <section className="grid gap-3 border-t border-border pt-6">
          <h3 className="flex items-center gap-2 text-sm font-semibold">
            <ShieldCheck className="size-4" aria-hidden />
            Who can join
          </h3>
          {data.whitelistEnabled ? (
            <p className="flex items-start gap-2 rounded-md border border-border px-3 py-2 text-xs">
              <CheckCircle2 className="mt-0.5 size-3.5 shrink-0 text-emerald-500" aria-hidden />
              The whitelist is on, so only players you have added can join. Manage it in the
              Players tab.
            </p>
          ) : (
            // Not a scold, and not automatic: turning the whitelist on for
            // somebody who meant to run an open server is its own bug.
            <p className="flex items-start gap-2 rounded-md border border-[var(--status-starting)]/40 bg-[var(--status-starting)]/10 px-3 py-2 text-xs">
              <AlertTriangle className="mt-0.5 size-3.5 shrink-0 text-[var(--status-starting)]" aria-hidden />
              <span>
                The whitelist is off. Anyone who has the address above can join
                {data.onlineMode ? "" : ", and online-mode is off, so they can pick any name"}.
              </span>
            </p>
          )}

          {data.whitelistEnabled ? null : (
            <div className="flex flex-wrap items-center gap-2">
              <Button
                size="sm"
                disabled={!stopped || enableWhitelist.isPending}
                onClick={() => enableWhitelist.mutate()}
              >
                {enableWhitelist.isPending ? "Saving…" : "Turn the whitelist on"}
              </Button>
              <span className="text-xs text-muted-foreground">
                {stopped
                  ? "Then add the players who should be allowed in, on the Players tab."
                  : "Stop the server first — a running server rewrites server.properties when it shuts down."}
              </span>
            </div>
          )}
        </section>
      </div>
    </div>
  );
}

function AddressRow({ entry }: { entry: NetAddress }) {
  return (
    <li className="flex flex-wrap items-center gap-2 rounded-md border border-border px-3 py-2">
      <Badge>{entry.network ?? KIND_LABEL[entry.kind]}</Badge>
      <code className="rounded bg-muted px-2 py-1 font-mono text-xs">{entry.joinable}</code>
      <CopyButton value={entry.joinable} />
      <span className="w-full text-xs text-muted-foreground sm:w-auto sm:flex-1">
        {entry.audience} <span className="opacity-60">({entry.interface})</span>
      </span>
    </li>
  );
}

const KIND_LABEL: Record<NetAddress["kind"], string> = {
  lan: "Same network",
  vpn: "VPN",
  public: "Internet",
  link_local: "No network",
  loopback: "This computer",
};

function CopyButton({ value }: { value: string }) {
  return (
    <Button
      variant="ghost"
      size="sm"
      aria-label={`Copy ${value}`}
      onClick={() => void copyToClipboard(value)}
    >
      <Copy /> Copy
    </Button>
  );
}

/**
 * The outside check's answer, including "could not tell".
 *
 * A failed check and a closed port look the same to a person unless the screen
 * says otherwise, and the two call for completely different next steps.
 */
function ExternalResult({ result }: { result: Reachability }) {
  const icon =
    result.reachable === true ? (
      <CheckCircle2 className="mt-0.5 size-3.5 shrink-0 text-emerald-500" aria-hidden />
    ) : result.reachable === false ? (
      <AlertTriangle className="mt-0.5 size-3.5 shrink-0 text-[var(--status-starting)]" aria-hidden />
    ) : (
      <HelpCircle className="mt-0.5 size-3.5 shrink-0 text-muted-foreground" aria-hidden />
    );

  return (
    <p className="mt-2 flex items-start gap-2 rounded-md border border-border px-3 py-2 text-xs">
      {icon}
      <span>
        {result.detail}{" "}
        <span className="opacity-60">Checked {result.askedAbout}.</span>
      </span>
    </p>
  );
}
