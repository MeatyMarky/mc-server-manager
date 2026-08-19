// The only place that calls `invoke`. One function per Tauri command, so the
// command surface is visible in a single file and every call site is typed.
import { invoke } from "@tauri-apps/api/core";

import type {
  AppErrorShape,
  AppInfo,
  ArchiveEntry,
  Backup,
  BackupOptions,
  Estimate,
  MetricsWindow,
  Sample,
  Schedule,
  ScheduleInput,
  SpaceCheck,
  BuildEntry,
  EulaStatus,
  JavaRuntime,
  JavaStatus,
  InstallPlan,
  KeyInfo,
  ModView,
  ModsView,
  Mutation,
  PackPlan,
  Project,
  SourceVersion,
  MutationReport,
  ParsedLine,
  PlayerLists,
  PropertiesUpdate,
  PropertiesView,
  SaveReport,
  StopStage,
  World,
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

  // Phase 4: configuration, players, worlds.
  propertiesRead: (id: number) => invoke<PropertiesView>("properties_read", { id }),
  propertiesWrite: (id: number, input: PropertiesUpdate) =>
    invoke<SaveReport>("properties_write", { id, input }),
  propertiesSchema: () => invoke<KeyInfo[]>("properties_schema"),

  playersRead: (id: number) => invoke<PlayerLists>("players_read", { id }),
  /** Every op/whitelist/ban change goes through this one command. */
  playersMutate: (id: number, mutation: Mutation) =>
    invoke<MutationReport>("players_mutate", { id, mutation }),
  /** Returns [uuid, fromMojang]; false means the offline UUID was derived. */
  playersResolveUuid: (name: string) =>
    invoke<[string, boolean]>("players_resolve_uuid", { name }),

  worldsList: (id: number) => invoke<World[]>("worlds_list", { id }),
  /** Returns a task id; the size arrives on task://done. */
  worldMeasure: (id: number, folder: string) =>
    invoke<string>("world_measure", { id, folder }),
  worldSwitch: (id: number, folder: string) =>
    invoke<void>("world_switch", { id, folder }),
  worldDelete: (id: number, folder: string) =>
    invoke<void>("world_delete", { id, folder }),
  worldExport: (id: number, folder: string, target: string) =>
    invoke<string>("world_export", { id, folder, target }),
  worldImport: (id: number, archive: string, folder: string | null) =>
    invoke<string>("world_import", { id, archive, folder }),

  // Phase 5: mods, plugins and packs.
  modsList: (id: number) => invoke<ModsView>("mods_list", { id }),
  modsSearch: (id: number, text: string, limit?: number, offset?: number) =>
    invoke<Project[]>("mods_search", { id, text, limit, offset }),
  modsVersions: (id: number, projectId: string) =>
    invoke<SourceVersion[]>("mods_versions", { id, projectId }),
  /** Resolves dependencies. Nothing is downloaded until the plan is confirmed. */
  modsPlan: (id: number, projectId: string, versionId: string | null) =>
    invoke<InstallPlan>("mods_plan", { id, projectId, versionId }),
  /** Returns a task id; progress arrives as task events. */
  modsInstall: (id: number, plan: InstallPlan) =>
    invoke<string>("mods_install", { id, plan }),
  modsSetEnabled: (id: number, fileName: string, enabled: boolean) =>
    invoke<ModsView>("mods_set_enabled", { id, fileName, enabled }),
  modsSetPinned: (id: number, fileName: string, pinned: boolean) =>
    invoke<ModsView>("mods_set_pinned", { id, fileName, pinned }),
  /** Resolves with the names of mods that depended on the removed one. */
  modsUninstall: (id: number, fileName: string) =>
    invoke<string[]>("mods_uninstall", { id, fileName }),
  modsInstallLocal: (id: number, path: string) =>
    invoke<ModView>("mods_install_local", { id, path }),
  modsCheckUpdates: (id: number) => invoke<ModsView>("mods_check_updates", { id }),

  mrpackPlan: (id: number, archive: string) =>
    invoke<PackPlan>("mrpack_plan", { id, archive }),
  mrpackImport: (id: number, archive: string) =>
    invoke<string>("mrpack_import", { id, archive }),

  // Phase 6: backups, schedules and metrics.
  backupsList: (id: number) => invoke<Backup[]>("backups_list", { id }),
  /** Size estimate plus the free-space verdict, shown before anything starts. */
  backupPlan: (id: number, options: BackupOptions) =>
    invoke<SpaceCheck>("backup_plan", { id, options }),
  backupEstimate: (id: number, options: BackupOptions) =>
    invoke<Estimate>("backup_estimate", { id, options }),
  /** Returns a task id; progress arrives as task events. */
  backupCreate: (id: number, options: BackupOptions) =>
    invoke<string>("backup_create", { id, options }),
  backupDelete: (backupId: number) => invoke<void>("backup_delete", { backupId }),
  backupPreview: (backupId: number) =>
    invoke<ArchiveEntry[]>("backup_preview", { backupId }),
  /** Returns a task id. The current state is archived before anything is written. */
  backupRestore: (backupId: number) => invoke<string>("backup_restore", { backupId }),
  backupsPrune: (id: number, keepCount: number | null, keepDays: number | null) =>
    invoke<number>("backups_prune", { id, keepCount, keepDays }),

  schedulesList: (id: number) => invoke<Schedule[]>("schedules_list", { id }),
  scheduleSave: (id: number, input: ScheduleInput) =>
    invoke<Schedule>("schedule_save", { id, input }),
  scheduleDelete: (scheduleId: number) =>
    invoke<void>("schedule_delete", { scheduleId }),
  scheduleRunNow: (scheduleId: number) =>
    invoke<void>("schedule_run_now", { scheduleId }),

  metricsRange: (id: number, window: MetricsWindow) =>
    invoke<Sample[]>("metrics_range", { id, window }),
  /** The heap the JVM is actually given, for the memory chart's ceiling. */
  metricsHeapBytes: (id: number) => invoke<number | null>("metrics_heap_bytes", { id }),
};
