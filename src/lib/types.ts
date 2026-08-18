// Re-export of the ts-rs bindings generated from the Rust DTOs by `cargo test`.
// Nothing in src/lib/bindings is hand-written; edit the Rust structs instead.
export type { AppInfo } from "./bindings/AppInfo";
export type { BannedIp } from "./bindings/BannedIp";
export type { BannedPlayer } from "./bindings/BannedPlayer";
export type { ConsoleEvent } from "./bindings/ConsoleEvent";
export type { Artifact } from "./bindings/Artifact";
export type { ArtifactKind } from "./bindings/ArtifactKind";
export type { BuildEntry } from "./bindings/BuildEntry";
export type { CloneInstanceInput } from "./bindings/CloneInstanceInput";
export type { CreateInstanceInput } from "./bindings/CreateInstanceInput";
export type { DeleteReport } from "./bindings/DeleteReport";
export type { DetectConfidence } from "./bindings/DetectConfidence";
export type { EulaStatus } from "./bindings/EulaStatus";
export type { ImportCandidate } from "./bindings/ImportCandidate";
export type { ImportInstanceInput } from "./bindings/ImportInstanceInput";
export type { InstanceManifest } from "./bindings/InstanceManifest";
export type { InstanceStatus } from "./bindings/InstanceStatus";
export type { InstanceStatusEvent } from "./bindings/InstanceStatusEvent";
export type { InstanceView } from "./bindings/InstanceView";
export type { JavaRuntime } from "./bindings/JavaRuntime";
export type { JavaSource } from "./bindings/JavaSource";
export type { JavaStatus } from "./bindings/JavaStatus";
export type { KeyInfo } from "./bindings/KeyInfo";
export type { Mutation } from "./bindings/Mutation";
export type { MutationReport } from "./bindings/MutationReport";
export type { MutationRoute } from "./bindings/MutationRoute";
export type { OpEntry } from "./bindings/OpEntry";
export type { LaunchKind } from "./bindings/LaunchKind";
export type { LogLevel } from "./bindings/LogLevel";
export type { ParsedLine } from "./bindings/ParsedLine";
export type { PlayerEvent } from "./bindings/PlayerEvent";
export type { PlayerLists } from "./bindings/PlayerLists";
export type { PropertiesUpdate } from "./bindings/PropertiesUpdate";
export type { PropertiesView } from "./bindings/PropertiesView";
export type { PropertyEntry } from "./bindings/PropertyEntry";
export type { SaveReport } from "./bindings/SaveReport";
export type { SeenPlayer } from "./bindings/SeenPlayer";
export type { QuitRequestedEvent } from "./bindings/QuitRequestedEvent";
export type { ServerType } from "./bindings/ServerType";
export type { StopStage } from "./bindings/StopStage";
export type { ValueKind } from "./bindings/ValueKind";
export type { WhitelistEntry } from "./bindings/WhitelistEntry";
export type { World } from "./bindings/World";
export type { TaskDoneEvent } from "./bindings/TaskDoneEvent";
export type { TaskProgressEvent } from "./bindings/TaskProgressEvent";
export type { VersionEntry } from "./bindings/VersionEntry";
export type { UpdateInstanceInput } from "./bindings/UpdateInstanceInput";

/// The shape every rejected Tauri command produces (see AppError in Rust).
export interface AppErrorShape {
  kind: string;
  message: string;
  /// Extra payload for errors that carry one, e.g. installer logs.
  detail?: { logPath?: string; logTail?: string; required?: number } | null;
}
