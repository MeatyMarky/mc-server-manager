import { Copy, FolderOpen, MoreVertical, Pencil, Trash2 } from "lucide-react";
import { useState } from "react";

import { Button } from "@/components/ui/button";
import {
  Badge,
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
  Tabs,
  TabsContent,
  TabsList,
  TabsTrigger,
} from "@/components/ui/misc";
import { StatusDot } from "@/features/instances/InstanceSidebar";
import { ipc } from "@/lib/ipc";
import { toastError } from "@/lib/toast";
import { SERVER_TYPE_LABEL, STATUS_LABEL } from "@/lib/status";
import type { InstanceView } from "@/lib/types";
import { ConfigTab } from "@/features/config/ConfigTab";
import { ConsoleTab } from "@/features/console/ConsoleTab";
import { ModsTab } from "@/features/mods/ModsTab";
import { NetworkingTab } from "@/features/network/NetworkingTab";
import { PlayersTab } from "@/features/players/PlayersTab";
import { BackupsTab } from "@/features/backups/BackupsTab";
import { WorldsTab } from "@/features/worlds/WorldsTab";
import { RunControls } from "@/features/console/RunControls";
import { CloneDialog, DeleteDialog, LocateBanner, RenameDialog } from "./dialogs";
import { PlaceholderTab } from "./tabs/PlaceholderTab";
import { SettingsTab } from "./tabs/SettingsTab";

const TABS = [
  { value: "console", label: "Console" },
  { value: "mods", label: "Mods" },
  { value: "config", label: "Config" },
  { value: "players", label: "Players" },
  { value: "worlds", label: "Worlds" },
  { value: "backups", label: "Backups" },
  { value: "networking", label: "Networking" },
  { value: "settings", label: "Settings" },
] as const;

export function InstanceDetail({ instance }: { instance: InstanceView }) {
  const [renaming, setRenaming] = useState(false);
  const [cloning, setCloning] = useState(false);
  const [deleting, setDeleting] = useState(false);
  // Kept here rather than inside the controls, so the reason a start failed is
  // still readable in the Console tab after the toast has gone.
  const [startError, setStartError] = useState<unknown>(null);

  const missing = instance.status === "missing";

  return (
    <section className="flex min-h-0 min-w-0 flex-1 flex-col p-6">
      <header className="flex flex-wrap items-start justify-between gap-4">
        <div className="min-w-0">
          <div className="flex items-center gap-2">
            <StatusDot status={instance.status} />
            <h2 className="truncate text-xl font-semibold">{instance.name}</h2>
          </div>
          <p className="mt-1 flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
            <Badge>{SERVER_TYPE_LABEL[instance.serverType]}</Badge>
            <Badge>{instance.mcVersion}</Badge>
            {instance.loaderVersion ? <Badge>build {instance.loaderVersion}</Badge> : null}
            <span>{STATUS_LABEL[instance.status]}</span>
            {instance.eulaAccepted ? null : <Badge>EULA not accepted</Badge>}
            {instance.installedAt ? null : <Badge>no server installed</Badge>}
          </p>
        </div>

        <div className="flex items-center gap-2">
          <RunControls instance={instance} onStartError={setStartError} />
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button variant="outline" size="icon" aria-label="Instance actions">
                <MoreVertical />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end">
              <DropdownMenuItem onSelect={() => setRenaming(true)}>
                <Pencil /> Rename
              </DropdownMenuItem>
              <DropdownMenuItem disabled={missing} onSelect={() => setCloning(true)}>
                <Copy /> Clone
              </DropdownMenuItem>
              <DropdownMenuItem
                disabled={missing}
                onSelect={() => {
                  ipc.instanceOpenFolder(instance.id).catch((error) => {
                    toastError(error);
                  });
                }}
              >
                <FolderOpen /> Open folder
              </DropdownMenuItem>
              <DropdownMenuSeparator />
              <DropdownMenuItem
                className="text-destructive focus:text-destructive"
                onSelect={() => setDeleting(true)}
              >
                <Trash2 /> Delete…
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        </div>
      </header>

      {missing ? (
        <div className="mt-4">
          <LocateBanner instance={instance} />
        </div>
      ) : null}

      <Tabs defaultValue="console" className="mt-6 flex min-h-0 flex-1 flex-col">
        <TabsList>
          {TABS.map((tab) => (
            <TabsTrigger key={tab.value} value={tab.value}>
              {tab.label}
            </TabsTrigger>
          ))}
        </TabsList>

        <TabsContent value="console" className="min-h-0">
          {missing ? (
            <PlaceholderTab
              title="Console"
              phase={4}
              description="Locate the instance folder to work with this server again."
            />
          ) : (
            <ConsoleTab instance={instance} startError={startError} />
          )}
        </TabsContent>
        <TabsContent value="mods" className="min-h-0">
          {missing ? (
            <PlaceholderTab
              title={instance.contentDir === "plugins" ? "Plugins" : "Mods"}
              phase={5}
              description="Locate the instance folder to manage its content."
            />
          ) : (
            <ModsTab instance={instance} />
          )}
        </TabsContent>
        <TabsContent value="config" className="min-h-0">
          {missing ? (
            <PlaceholderTab
              title="Config"
              phase={4}
              description="Locate the instance folder to edit its configuration."
            />
          ) : (
            <ConfigTab instance={instance} />
          )}
        </TabsContent>
        <TabsContent value="players" className="min-h-0">
          {missing ? (
            <PlaceholderTab
              title="Players"
              phase={4}
              description="Locate the instance folder to manage its players."
            />
          ) : (
            <PlayersTab instance={instance} />
          )}
        </TabsContent>
        <TabsContent value="worlds" className="min-h-0">
          {missing ? (
            <PlaceholderTab
              title="Worlds"
              phase={4}
              description="Locate the instance folder to manage its worlds."
            />
          ) : (
            <WorldsTab instance={instance} />
          )}
        </TabsContent>
        <TabsContent value="backups" className="min-h-0">
          {missing ? (
            <PlaceholderTab
              title="Backups"
              phase={6}
              description="Locate the instance folder to back it up or restore it."
            />
          ) : (
            <BackupsTab instance={instance} />
          )}
        </TabsContent>
        <TabsContent value="networking" className="min-h-0">
          {missing ? (
            <PlaceholderTab
              title="Networking"
              phase={10}
              description="Locate the instance folder to see how people reach this server."
            />
          ) : (
            <NetworkingTab instance={instance} />
          )}
        </TabsContent>
        <TabsContent value="settings">
          <SettingsTab instance={instance} />
        </TabsContent>
      </Tabs>

      <RenameDialog instance={instance} open={renaming} onOpenChange={setRenaming} />
      <CloneDialog instance={instance} open={cloning} onOpenChange={setCloning} />
      <DeleteDialog instance={instance} open={deleting} onOpenChange={setDeleting} />
    </section>
  );
}
