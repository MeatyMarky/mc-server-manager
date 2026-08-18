// Typed wrappers around Tauri events. The UI subscribes to these; it never polls.
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import type {
  ConsoleEvent,
  InstanceStatusEvent,
  PlayerEvent,
  QuitRequestedEvent,
  TaskDoneEvent,
  TaskProgressEvent,
} from "./types";

export const EVENTS = {
  instancesChanged: "instances://changed",
  instanceStatus: "instance://status",
  quitRequested: "app://quit-requested",
  console: "instance://console",
  player: "instance://player",
  taskProgress: "task://progress",
  taskDone: "task://done",
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

export function onTaskProgress(
  handler: (payload: TaskProgressEvent) => void,
): Promise<UnlistenFn> {
  return listen<TaskProgressEvent>(EVENTS.taskProgress, (event) => handler(event.payload));
}

export function onTaskDone(handler: (payload: TaskDoneEvent) => void): Promise<UnlistenFn> {
  return listen<TaskDoneEvent>(EVENTS.taskDone, (event) => handler(event.payload));
}

export function onConsoleLines(handler: (payload: ConsoleEvent) => void): Promise<UnlistenFn> {
  return listen<ConsoleEvent>(EVENTS.console, (event) => handler(event.payload));
}

export function onPlayerEvent(handler: (payload: PlayerEvent) => void): Promise<UnlistenFn> {
  return listen<PlayerEvent>(EVENTS.player, (event) => handler(event.payload));
}
