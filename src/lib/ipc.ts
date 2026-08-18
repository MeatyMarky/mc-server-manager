// The only place that calls `invoke`. One function per Tauri command, so the
// command surface is visible in a single file and every call site is typed.
import { invoke } from "@tauri-apps/api/core";

import type {
  AppErrorShape,
  AppInfo,
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
};
