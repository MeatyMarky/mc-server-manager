// The only place that calls `invoke`. One function per Tauri command, so the
// command surface is visible in a single file and every call site is typed.
import { invoke } from "@tauri-apps/api/core";

import type {
  AppErrorShape,
  AppInfo,
  BuildEntry,
  EulaStatus,
  JavaRuntime,
  JavaStatus,
  ParsedLine,
  StopStage,
  ServerType,
  VersionEntry,
  CloneInstanceInput,
  CreateInstanceInput,
  DeleteReport,
  ImportCandidate,
  ImportInstanceInput,
  InstanceView,
  UpdateInstanceInput,
} from "./types";

/** Rejections from Rust are `{ kind, message }`; anything else is a bug or a panic. */
export function isAppError(error: unknown): error is AppErrorShape {
  return (
    typeof error === "object" &&
    error !== null &&
    "kind" in error &&
    "message" in error &&
    typeof (error as AppErrorShape).message === "string"
  );
}

export function errorMessage(error: unknown): string {
  if (isAppError(error)) return error.message;
  if (error instanceof Error) return error.message;
  return String(error);
}

export const ipc = {
  appInfo: () => invoke<AppInfo>("app_info"),
  appQuit: () => invoke<void>("app_quit"),
  liveInstances: () => invoke<string[]>("live_instances"),

  settingsGetAll: () => invoke<Record<string, string>>("settings_get_all"),
  settingsSet: (key: string, value: string) =>
    invoke<void>("settings_set", { key, value }),

  instanceList: () => invoke<InstanceView[]>("instance_list"),
  instanceGet: (id: number) => invoke<InstanceView>("instance_get", { id }),
  instanceCreate: (input: CreateInstanceInput) =>
    invoke<InstanceView>("instance_create", { input }),
  instanceClone: (input: CloneInstanceInput) =>
    invoke<InstanceView>("instance_clone", { input }),
  instanceRename: (id: number, name: string) =>
    invoke<InstanceView>("instance_rename", { id, name }),
  instanceUpdate: (id: number, input: UpdateInstanceInput) =>
    invoke<InstanceView>("instance_update", { id, input }),
  instanceDelete: (id: number, deleteFiles: boolean) =>
    invoke<DeleteReport>("instance_delete", { id, deleteFiles }),
  instanceLocate: (id: number, path: string) =>
    invoke<InstanceView>("instance_locate", { id, path }),
  /** Path building lives in Rust; this only previews the folder for the dialog. */
  instanceSuggestPath: (root: string, name: string) =>
    invoke<string>("instance_suggest_path", { root, name }),
  instanceImportDetect: (path: string) =>
    invoke<ImportCandidate>("instance_import_detect", { path }),
  instanceImport: (input: ImportInstanceInput) =>
    invoke<InstanceView>("instance_import", { input }),
  instanceForceStop: (id: number) =>
    invoke<InstanceView>("instance_force_stop", { id }),
  instanceOpenFolder: (id: number) => invoke<void>("instance_open_folder", { id }),

  // Phase 2: getting a server into an instance.
  providerVersions: (serverType: ServerType) =>
    invoke<VersionEntry[]>("provider_versions", { serverType }),
  providerBuilds: (serverType: ServerType, mcVersion: string) =>
    invoke<BuildEntry[]>("provider_builds", { serverType, mcVersion }),
  /** Returns a task id; progress and the outcome arrive as events. */
  installServer: (id: number, mcVersion: string, build: string | null) =>
    invoke<string>("install_server", { id, mcVersion, build }),
  taskCancel: (taskId: string) => invoke<boolean>("task_cancel", { taskId }),
  readInstallerLog: (path: string) => invoke<string>("read_installer_log", { path }),

  eulaGet: (id: number) => invoke<EulaStatus>("eula_get", { id }),
  eulaSet: (id: number, accepted: boolean) =>
    invoke<EulaStatus>("eula_set", { id, accepted }),

  javaList: () => invoke<JavaRuntime[]>("java_list"),
  javaRescan: () => invoke<JavaRuntime[]>("java_rescan"),
  javaAddManual: (path: string) => invoke<JavaRuntime>("java_add_manual", { path }),
  javaStatus: (id: number) => invoke<JavaStatus>("java_status", { id }),
  javaRequiredFor: (mcVersion: string) =>
    invoke<number>("java_required_for", { mcVersion }),

  // Phase 3: process control.
  instanceStart: (id: number) => invoke<InstanceView>("instance_start", { id }),
  /** Resolves with how far the stop had to go: graceful, terminated or killed. */
  instanceStop: (id: number) => invoke<StopStage>("instance_stop", { id }),
  instanceKill: (id: number) => invoke<StopStage>("instance_kill", { id }),
  instanceRestart: (id: number) => invoke<InstanceView>("instance_restart", { id }),
  instanceSendCommand: (id: number, command: string) =>
    invoke<void>("instance_send_command", { id, command }),
  consoleTail: (id: number, count?: number) =>
    invoke<ParsedLine[]>("console_tail", { id, count }),
  commandHistory: (id: number) => invoke<string[]>("command_history", { id }),
  /** null when the port is free; otherwise a sentence naming the conflict. */
  portStatus: (id: number) => invoke<string | null>("port_status", { id }),
};
