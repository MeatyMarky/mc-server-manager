// Typed wrappers around Tauri events. The UI subscribes to these; it never polls.
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import type { InstanceStatusEvent, QuitRequestedEvent } from "./types";

export const EVENTS = {
  instancesChanged: "instances://changed",
  instanceStatus: "instance://status",
  quitRequested: "app://quit-requested",
} as const;

export function onInstancesChanged(handler: () => void): Promise<UnlistenFn> {
  return listen<null>(EVENTS.instancesChanged, () => handler());
}

export function onInstanceStatus(
  handler: (payload: InstanceStatusEvent) => void,
): Promise<UnlistenFn> {
  return listen<InstanceStatusEvent>(EVENTS.instanceStatus, (event) =>
    handler(event.payload),
  );
}

export function onQuitRequested(
  handler: (payload: QuitRequestedEvent) => void,
): Promise<UnlistenFn> {
  return listen<QuitRequestedEvent>(EVENTS.quitRequested, (event) =>
    handler(event.payload),
  );
}
