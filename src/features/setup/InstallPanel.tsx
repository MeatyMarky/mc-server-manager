import { Download, RefreshCw, X } from "lucide-react";
import { useEffect, useState } from "react";

import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/dialog";
import { Select } from "@/components/ui/input";
import { Badge } from "@/components/ui/misc";
import { formatBytes, progressPercent } from "@/lib/format";
import { SERVER_TYPE_LABEL } from "@/lib/status";
import type { InstanceView } from "@/lib/types";
import { EulaCard } from "./EulaCard";
import { InstallerFailureDialog } from "./InstallerFailureDialog";
import { JavaWarning } from "./JavaWarning";
import { useInstall, useProviderBuilds, useProviderVersions } from "./queries";

const PHASE_LABEL: Record<string, string> = {
  resolve: "Resolving version",
  download: "Downloading",
  install: "Installing",
  finalize: "Finishing up",
};

/**
 * Installs (or reinstalls) the server for one instance. Shown as the main
 * content of the Console tab until a server is present.
 */
export function InstallPanel({ instance }: { instance: InstanceView }) {
  const install = useInstall(instance.id);
  const versions = useProviderVersions(instance.serverType);
  const [mcVersion, setMcVersion] = useState(instance.mcVersion);
  const builds = useProviderBuilds(instance.serverType, mcVersion);
  const [build, setBuild] = useState<string>("");

  // A version change invalidates whichever build was picked for the old one.
  useEffect(() => setBuild(""), [mcVersion]);

  const installed = Boolean(instance.installedAt);
  const progress = install.progress;
  const percent = progress ? progressPercent(progress.done, progress.total) : null;

  return (
    <div className="grid gap-4">
      <div className="rounded-lg border border-border p-4">
        <div className="flex flex-wrap items-start justify-between gap-3">
          <div>
            <h3 className="text-sm font-semibold">
              {installed ? "Server files" : "No server installed yet"}
            </h3>
            <p className="mt-1 text-sm text-muted-foreground">
              {installed
                ? `${SERVER_TYPE_LABEL[instance.serverType]} ${instance.mcVersion}${
                    instance.loaderVersion ? ` build ${instance.loaderVersion}` : ""
                  } is installed. Reinstalling replaces the server files and leaves worlds alone.`
                : `Download the ${SERVER_TYPE_LABEL[instance.serverType]} server for this instance. Downloads resume if interrupted and are checksum-verified before use.`}
            </p>
          </div>
          {installed ? <Badge>installed</Badge> : null}
        </div>

        <div className="mt-4 grid gap-4 sm:grid-cols-2">
          <div className="grid gap-2">
            <Label htmlFor="install-version">Minecraft version</Label>
            <Select
              id="install-version"
              value={mcVersion}
              disabled={versions.isLoading || install.isInstalling}
              onChange={(event) => setMcVersion(event.target.value)}
            >
              {versions.data?.some((v) => v.id === mcVersion) ? null : (
                <option value={mcVersion}>{mcVersion}</option>
              )}
              {versions.data?.map((version) => (
                <option key={version.id} value={version.id}>
                  {version.id}
                </option>
              ))}
            </Select>
            {versions.isError ? (
              <p className="text-xs text-destructive">
                Could not reach the version API. You can still type a version and retry.
              </p>
            ) : null}
          </div>

          {instance.serverType === "vanilla" ? null : (
            <div className="grid gap-2">
              <Label htmlFor="install-build">Build</Label>
              <Select
                id="install-build"
                value={build}
                disabled={builds.isLoading || install.isInstalling}
                onChange={(event) => setBuild(event.target.value)}
              >
                <option value="">Newest stable</option>
                {builds.data?.map((entry) => (
                  <option key={entry.id} value={entry.id}>
                    {entry.id}
                    {entry.label ? ` (${entry.label})` : ""}
                  </option>
                ))}
              </Select>
            </div>
          )}
        </div>

        {progress ? (
          <div className="mt-4 grid gap-2">
            <div className="flex items-center justify-between text-xs text-muted-foreground">
              <span>
                {PHASE_LABEL[progress.phase] ?? progress.phase}: {progress.message}
              </span>
              <span>
                {progress.total
                  ? `${formatBytes(progress.done)} / ${formatBytes(progress.total)}`
                  : formatBytes(progress.done)}
              </span>
            </div>
            <div
              className="h-2 w-full overflow-hidden rounded-full bg-muted"
              role="progressbar"
              aria-valuenow={percent ?? undefined}
              aria-valuemin={0}
              aria-valuemax={100}
              aria-label="Install progress"
            >
              <div
                className="h-full rounded-full bg-primary transition-all"
                style={{ width: percent === null ? "35%" : `${percent}%` }}
              />
            </div>
            <div>
              <Button variant="outline" size="sm" onClick={() => void install.cancel()}>
                <X /> Cancel
              </Button>
            </div>
          </div>
        ) : (
          <div className="mt-4 flex items-center gap-2">
            <Button
              onClick={() => void install.start(mcVersion, build || null)}
              disabled={!mcVersion}
            >
              {installed ? <RefreshCw /> : <Download />}
              {installed ? "Reinstall" : "Install server"}
            </Button>
            {instance.installedAt ? (
              <span className="text-xs text-muted-foreground">
                Installed {new Date(instance.installedAt).toLocaleString()}
              </span>
            ) : null}
          </div>
        )}
      </div>

      <JavaWarning instance={instance} />
      <EulaCard instance={instance} />

      <InstallerFailureDialog failure={install.failure} onClose={install.dismissFailure} />
    </div>
  );
}
