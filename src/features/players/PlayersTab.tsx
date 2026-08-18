import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Ban, Plus, Shield, ShieldOff, UserCheck, UserX } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/dialog";
import { Input, Select } from "@/components/ui/input";
import { Badge } from "@/components/ui/misc";
import { errorMessage, ipc } from "@/lib/ipc";
import type { InstanceView, Mutation } from "@/lib/types";

/** How a change was carried out, so the user knows the running server was told. */
const ROUTE_NOTE: Record<string, string> = {
  command: "Sent to the running server; its files were re-read.",
  file: "Written to the server's JSON file.",
};

export function PlayersTab({ instance }: { instance: InstanceView }) {
  const queryClient = useQueryClient();
  const [name, setName] = useState("");
  const [opLevel, setOpLevel] = useState("4");
  const [banReason, setBanReason] = useState("");
  const [ip, setIp] = useState("");

  const lists = useQuery({
    queryKey: ["players", instance.id],
    queryFn: () => ipc.playersRead(instance.id),
  });

  const mutate = useMutation({
    mutationFn: (mutation: Mutation) => ipc.playersMutate(instance.id, mutation),
    onSuccess: (report) => {
      void queryClient.invalidateQueries({ queryKey: ["players", instance.id] });
      toast.success("Player list updated", {
        description: report.command
          ? `${report.command} — ${ROUTE_NOTE.command}`
          : ROUTE_NOTE.file,
      });
    },
    onError: (error: unknown) => toast.error(errorMessage(error)),
  });

  const data = lists.data;
  if (lists.isLoading || !data) {
    return <p className="text-sm text-muted-foreground">Reading player lists…</p>;
  }

  const player = name.trim();
  const act = (mutation: Mutation) => mutate.mutate(mutation);

  return (
    <div className="flex h-full min-h-0 flex-col gap-4 overflow-y-auto pr-1">
      <div className="rounded-lg border border-border p-4">
        <h3 className="text-sm font-semibold">Add a player</h3>
        <p className="mt-1 text-sm text-muted-foreground">
          {data.running
            ? "The server is running, so these go through its console and the files are re-read afterwards."
            : "The server is stopped, so these are written straight to its JSON files."}
        </p>

        <div className="mt-3 flex flex-wrap items-end gap-2">
          <div className="grid gap-1.5">
            <Label htmlFor="player-name">Player name</Label>
            <Input
              id="player-name"
              className="w-48"
              placeholder="Notch"
              value={name}
              onChange={(event) => setName(event.target.value)}
            />
          </div>
          <div className="grid gap-1.5">
            <Label htmlFor="op-level">Op level</Label>
            <Select
              id="op-level"
              className="w-24"
              value={opLevel}
              onChange={(event) => setOpLevel(event.target.value)}
            >
              {["1", "2", "3", "4"].map((level) => (
                <option key={level} value={level}>
                  {level}
                </option>
              ))}
            </Select>
          </div>
          <Button
            size="sm"
            disabled={!player || mutate.isPending}
            onClick={() => act({ action: "op", player, level: Number(opLevel) })}
          >
            <Shield /> Op
          </Button>
          <Button
            size="sm"
            variant="outline"
            disabled={!player || mutate.isPending}
            onClick={() => act({ action: "whitelist_add", player })}
          >
            <UserCheck /> Whitelist
          </Button>
          <div className="grid gap-1.5">
            <Label htmlFor="ban-reason">Ban reason</Label>
            <Input
              id="ban-reason"
              className="w-56"
              placeholder="optional"
              value={banReason}
              onChange={(event) => setBanReason(event.target.value)}
            />
          </div>
          <Button
            size="sm"
            variant="destructive"
            disabled={!player || mutate.isPending}
            onClick={() => act({ action: "ban", player, reason: banReason || null })}
          >
            <Ban /> Ban
          </Button>
        </div>
      </div>

      <ListCard
        title="Operators"
        empty="Nobody is an operator."
        rows={data.ops.map((entry) => ({
          key: entry.uuid,
          label: entry.name,
          detail: `level ${entry.level} · ${entry.uuid}`,
          action: (
            <Button
              size="sm"
              variant="ghost"
              onClick={() => act({ action: "deop", player: entry.name })}
            >
              <ShieldOff /> Deop
            </Button>
          ),
        }))}
      />

      <ListCard
        title="Whitelist"
        badge={data.whitelistEnabled ? "enforced" : "not enforced"}
        empty="The whitelist is empty."
        rows={data.whitelist.map((entry) => ({
          key: entry.uuid,
          label: entry.name,
          detail: entry.uuid,
          action: (
            <Button
              size="sm"
              variant="ghost"
              onClick={() => act({ action: "whitelist_remove", player: entry.name })}
            >
              <UserX /> Remove
            </Button>
          ),
        }))}
      />

      <ListCard
        title="Banned players"
        empty="Nobody is banned."
        rows={data.bannedPlayers.map((entry) => ({
          key: entry.uuid,
          label: entry.name,
          detail: [entry.reason, entry.created].filter(Boolean).join(" · "),
          action: (
            <Button
              size="sm"
              variant="ghost"
              onClick={() => act({ action: "pardon", player: entry.name })}
            >
              Pardon
            </Button>
          ),
        }))}
      />

      <div className="rounded-lg border border-border p-4">
        <h3 className="text-sm font-semibold">Banned IP addresses</h3>
        <div className="mt-3 flex flex-wrap items-end gap-2">
          <div className="grid gap-1.5">
            <Label htmlFor="ban-ip">IP address</Label>
            <Input
              id="ban-ip"
              className="w-48 font-mono text-xs"
              placeholder="203.0.113.7"
              value={ip}
              onChange={(event) => setIp(event.target.value)}
            />
          </div>
          <Button
            size="sm"
            variant="destructive"
            disabled={!ip.trim() || mutate.isPending}
            onClick={() => act({ action: "ban_ip", ip: ip.trim(), reason: banReason || null })}
          >
            <Plus /> Ban IP
          </Button>
        </div>

        {data.bannedIps.length === 0 ? (
          <p className="mt-3 text-sm text-muted-foreground">No addresses are banned.</p>
        ) : (
          <ul className="mt-3 grid gap-2">
            {data.bannedIps.map((entry) => (
              <li
                key={entry.ip}
                className="flex items-center justify-between gap-3 rounded-md border border-border px-3 py-2"
              >
                <span className="min-w-0">
                  <span className="block truncate font-mono text-sm">{entry.ip}</span>
                  <span className="block truncate text-xs text-muted-foreground">
                    {[entry.reason, entry.created].filter(Boolean).join(" · ")}
                  </span>
                </span>
                <Button
                  size="sm"
                  variant="ghost"
                  onClick={() => act({ action: "pardon_ip", ip: entry.ip })}
                >
                  Pardon
                </Button>
              </li>
            ))}
          </ul>
        )}
      </div>

      <div className="rounded-lg border border-border p-4">
        <h3 className="text-sm font-semibold">Seen on this server</h3>
        <p className="mt-1 text-sm text-muted-foreground">
          Built from the console as players join, so you can act on someone without typing
          their name.
        </p>

        {data.seen.length === 0 ? (
          <p className="mt-3 text-sm text-muted-foreground">Nobody has joined yet.</p>
        ) : (
          <ul className="mt-3 grid gap-2">
            {data.seen.map((entry) => (
              <li
                key={entry.uuid}
                className="flex flex-wrap items-center justify-between gap-2 rounded-md border border-border px-3 py-2"
              >
                <span className="min-w-0">
                  <span className="block truncate text-sm font-medium">{entry.name}</span>
                  <span className="block truncate text-xs text-muted-foreground">
                    last seen {new Date(entry.lastSeen).toLocaleString()}
                  </span>
                </span>
                <span className="flex gap-1">
                  <Button
                    size="sm"
                    variant="ghost"
                    onClick={() => act({ action: "op", player: entry.name, level: 4 })}
                  >
                    Op
                  </Button>
                  <Button
                    size="sm"
                    variant="ghost"
                    onClick={() => act({ action: "whitelist_add", player: entry.name })}
                  >
                    Whitelist
                  </Button>
                  <Button
                    size="sm"
                    variant="ghost"
                    className="text-destructive"
                    onClick={() => act({ action: "ban", player: entry.name, reason: null })}
                  >
                    Ban
                  </Button>
                </span>
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}

function ListCard({
  title,
  badge,
  empty,
  rows,
}: {
  title: string;
  badge?: string;
  empty: string;
  rows: { key: string; label: string; detail: string; action: React.ReactNode }[];
}) {
  return (
    <div className="rounded-lg border border-border p-4">
      <h3 className="flex items-center gap-2 text-sm font-semibold">
        {title}
        {badge ? <Badge>{badge}</Badge> : null}
      </h3>

      {rows.length === 0 ? (
        <p className="mt-3 text-sm text-muted-foreground">{empty}</p>
      ) : (
        <ul className="mt-3 grid gap-2">
          {rows.map((row) => (
            <li
              key={row.key}
              className="flex items-center justify-between gap-3 rounded-md border border-border px-3 py-2"
            >
              <span className="min-w-0">
                <span className="block truncate text-sm font-medium">{row.label}</span>
                <span className="block truncate font-mono text-xs text-muted-foreground">
                  {row.detail}
                </span>
              </span>
              {row.action}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
