// Re-export of the ts-rs bindings generated from the Rust DTOs by `cargo test`.
// Nothing in src/lib/bindings is hand-written; edit the Rust structs instead.
export type { AppInfo } from "./bindings/AppInfo";
export type { CloneInstanceInput } from "./bindings/CloneInstanceInput";
export type { CreateInstanceInput } from "./bindings/CreateInstanceInput";
export type { DeleteReport } from "./bindings/DeleteReport";
export type { DetectConfidence } from "./bindings/DetectConfidence";
export type { ImportCandidate } from "./bindings/ImportCandidate";
export type { ImportInstanceInput } from "./bindings/ImportInstanceInput";
export type { InstanceManifest } from "./bindings/InstanceManifest";
export type { InstanceStatus } from "./bindings/InstanceStatus";
export type { InstanceStatusEvent } from "./bindings/InstanceStatusEvent";
export type { InstanceView } from "./bindings/InstanceView";
export type { LaunchKind } from "./bindings/LaunchKind";
export type { QuitRequestedEvent } from "./bindings/QuitRequestedEvent";
export type { ServerType } from "./bindings/ServerType";
export type { UpdateInstanceInput } from "./bindings/UpdateInstanceInput";

/// The shape every rejected Tauri command produces (see AppError in Rust).
export interface AppErrorShape {
  kind: string;
  message: string;
}
