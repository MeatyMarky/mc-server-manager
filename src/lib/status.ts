// Presentation rules for instance status. Pure, so they can be tested without
// a backend and reused by the sidebar, the header and the quit dialog.
import type { InstanceStatus, ServerType } from "./types";

export const STATUS_LABEL: Record<InstanceStatus, string> = {
  stopped: "Stopped",
  starting: "Starting",
  running: "Running",
  stopping: "Stopping",
  crashed: "Crashed",
  unmanaged: "Running, console unavailable",
  missing: "Folder missing",
};

export const STATUS_COLOR: Record<InstanceStatus, string> = {
  stopped: "var(--status-stopped)",
  starting: "var(--status-starting)",
  running: "var(--status-running)",
  stopping: "var(--status-stopping)",
  crashed: "var(--status-crashed)",
  unmanaged: "var(--status-unmanaged)",
  missing: "var(--status-missing)",
};

/** Starting and stopping pulse; everything else is steady. */
export function statusPulses(status: InstanceStatus): boolean {
  return status === "starting" || status === "stopping";
}

/** A live instance cannot be deleted, cloned or edited. */
export function statusIsLive(status: InstanceStatus): boolean {
  return (
    status === "starting" ||
    status === "running" ||
    status === "stopping" ||
    status === "unmanaged"
  );
}

/** A missing folder is recoverable: the UI offers "Locate folder…", not an error. */
export function needsLocating(status: InstanceStatus): boolean {
  return status === "missing";
}

export const SERVER_TYPE_LABEL: Record<ServerType, string> = {
  vanilla: "Vanilla",
  paper: "Paper",
  purpur: "Purpur",
  fabric: "Fabric",
  forge: "Forge",
  neo_forge: "NeoForge",
};

export const SERVER_TYPES: ServerType[] = [
  "vanilla",
  "paper",
  "purpur",
  "fabric",
  "forge",
  "neo_forge",
];
