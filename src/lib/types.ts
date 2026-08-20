// Re-export of the ts-rs bindings generated from the Rust DTOs by `cargo test`.
// Nothing in src/lib/bindings is hand-written; edit the Rust structs instead.
export type { AppInfo } from "./bindings/AppInfo";
export type { BannedIp } from "./bindings/BannedIp";
export type { BannedPlayer } from "./bindings/BannedPlayer";
export type { ConsoleEvent } from "./bindings/ConsoleEvent";
export type { Dependency } from "./bindings/Dependency";
export type { DependencyKind } from "./bindings/DependencyKind";
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
export type { ScanInfo } from "./bindings/ScanInfo";
export type { VersionKind } from "./bindings/VersionKind";
export type { NetworkView } from "./bindings/NetworkView";
export type { MapAddresses } from "./bindings/MapAddresses";
export type { MapKind } from "./bindings/MapKind";
export type { MapStatus } from "./bindings/MapStatus";
export type { NetAddress } from "./bindings/NetAddress";
export type { AddressKind } from "./bindings/AddressKind";
export type { Gateway } from "./bindings/Gateway";
export type { LocalPort } from "./bindings/LocalPort";
export type { Reachability } from "./bindings/Reachability";
export type { MappingResult } from "./bindings/MappingResult";
export type { PublicAddress } from "./bindings/PublicAddress";
export type { InstallPlan } from "./bindings/InstallPlan";
export type { InstalledMod } from "./bindings/InstalledMod";
export type { JarMetadata } from "./bindings/JarMetadata";
export type { Loader } from "./bindings/Loader";
export type { Mismatch } from "./bindings/Mismatch";
export type { ModView } from "./bindings/ModView";
export type { ModsView } from "./bindings/ModsView";
export type { KeyInfo } from "./bindings/KeyInfo";
export type { Mutation } from "./bindings/Mutation";
export type { MutationReport } from "./bindings/MutationReport";
export type { MutationRoute } from "./bindings/MutationRoute";
export type { OpEntry } from "./bindings/OpEntry";
export type { LaunchKind } from "./bindings/LaunchKind";
export type { LogLevel } from "./bindings/LogLevel";
export type { ParsedLine } from "./bindings/ParsedLine";
export type { PlayerEvent } from "./bindings/PlayerEvent";
export type { OptionalDependency } from "./bindings/OptionalDependency";
export type { PackFile } from "./bindings/PackFile";
export type { PackIndex } from "./bindings/PackIndex";
export type { PackPlan } from "./bindings/PackPlan";
export type { PlannedMod } from "./bindings/PlannedMod";
export type { Project } from "./bindings/Project";
export type { PlayerLists } from "./bindings/PlayerLists";
export type { PropertiesUpdate } from "./bindings/PropertiesUpdate";
export type { PropertiesView } from "./bindings/PropertiesView";
export type { PropertyEntry } from "./bindings/PropertyEntry";
export type { SaveReport } from "./bindings/SaveReport";
export type { SeenPlayer } from "./bindings/SeenPlayer";
export type { QuitRequestedEvent } from "./bindings/QuitRequestedEvent";
export type { ServerType } from "./bindings/ServerType";
export type { StopStage } from "./bindings/StopStage";
export type { SearchQuery } from "./bindings/SearchQuery";
export type { Side } from "./bindings/Side";
export type { SourceFile } from "./bindings/SourceFile";
export type { SourceId } from "./bindings/SourceId";
export type { SourceVersion } from "./bindings/SourceVersion";
export type { ValueKind } from "./bindings/ValueKind";
export type { WhitelistEntry } from "./bindings/WhitelistEntry";
export type { World } from "./bindings/World";
export type { TaskDoneEvent } from "./bindings/TaskDoneEvent";
export type { TaskProgressEvent } from "./bindings/TaskProgressEvent";
export type { VersionEntry } from "./bindings/VersionEntry";
export type { UpdateInstanceInput } from "./bindings/UpdateInstanceInput";

/// The shape every rejected Tauri command produces (see AppError in Rust).
///
/// `message` is written for a person; `technical` is the developer text kept
/// behind a "details" expander. Never show `technical` on its own.
export interface AppErrorShape {
  kind: string;
  message: string;
  /// What to do about it, when there is something to do.
  hint?: string | null;
  /// The Display text of the Rust error, for bug reports.
  technical?: string | null;
  /// Extra payload for errors that carry one, e.g. installer logs.
  detail?: {
    logPath?: string;
    logTail?: string;
    required?: number;
    found?: number;
    mcVersion?: string;
    port?: number;
    takenBy?: string | null;
    host?: string;
    retryAfterSeconds?: number;
  } | null;
}

// Phase 6: backups, schedules and resource metrics.
export type { ArchiveEntry } from "./bindings/ArchiveEntry";
export type { Backup } from "./bindings/Backup";
export type { BackupOptions } from "./bindings/BackupOptions";
export type { Estimate } from "./bindings/Estimate";
export type { Format } from "./bindings/Format";
export type { MetricsEvent } from "./bindings/MetricsEvent";
export type { Sample } from "./bindings/Sample";
export type { Schedule } from "./bindings/Schedule";
export type { ScheduleInput } from "./bindings/ScheduleInput";
export type { Scope } from "./bindings/Scope";
export type { SpaceCheck } from "./bindings/SpaceCheck";
export type { Window as MetricsWindow } from "./bindings/Window";

// Phase 7: about, first-run readiness and the problem report.
export type { BuildInfo } from "./bindings/BuildInfo";
export type { Readiness } from "./bindings/Readiness";
export type { ReportPart } from "./bindings/ReportPart";
export type { ReportPreview } from "./bindings/ReportPreview";

// Managed JDKs: runtimes this app downloads so a server never depends on what
// happens to be installed.
export type { DownloadOffer } from "./bindings/DownloadOffer";
export type { JavaPlan } from "./bindings/JavaPlan";
export type { ManagedRuntime } from "./bindings/ManagedRuntime";
export type { Origin as JavaOrigin } from "./bindings/Origin";

// The startup self-check.
export type { Health } from "./bindings/Health";
export type { HealthCheck } from "./bindings/HealthCheck";
export type { HealthStatus } from "./bindings/HealthStatus";

// Phase 8: the mod browser.
export type { Category } from "./bindings/Category";
export type { ContentType } from "./bindings/ContentType";
export type { ContentTypeOption } from "./bindings/ContentTypeOption";
export type { SearchPage } from "./bindings/SearchPage";
export type { SortBy } from "./bindings/SortBy";
export type { SourceStatus } from "./bindings/SourceStatus";

// Phase 9: modpacks, browsed for a server.
export type { InstallPackInput } from "./bindings/InstallPackInput";
export type { PackDetail } from "./bindings/PackDetail";
export type { ServerSupport } from "./bindings/ServerSupport";
