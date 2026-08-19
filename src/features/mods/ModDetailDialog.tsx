import { useQuery } from "@tanstack/react-query";
import { convertFileSrc } from "@tauri-apps/api/core";
import { Bug, Code2, ExternalLink, Package, Scale } from "lucide-react";
import { useEffect, useState } from "react";

import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  Label,
} from "@/components/ui/dialog";
import { Select } from "@/components/ui/input";
import { Badge } from "@/components/ui/misc";
import { ErrorNotice } from "@/components/ui/ErrorNotice";
import { ipc } from "@/lib/ipc";
import { openExternal } from "@/lib/external";
import type { InstanceView, Project, SourceVersion } from "@/lib/types";

import { compactCount, relativeTime } from "./browser";
import {
  installLabel,
  mismatchReason,
  newestCompatible,
  newestFirst,
  publishedLabel,
  versionLabel,
} from "./versions";

/**
 * One project in full, with the choice of which file to install.
 *
 * The card's Install is the shortcut for "the newest that fits"; this is where
 * a particular version is chosen — including an older one, which is a thing
 * people need and most launchers do not offer.
 */
export function ModDetailDialog({
  instance,
  project,
  loader,
  installedVersionId,
  onClose,
  onInstall,
}: {
  instance: InstanceView;
  project: Project | null;
  /// The instance's loader, for deciding what fits.
  loader: string | null;
  /// The version already installed for this project, if any.
  installedVersionId: string | null;
  onClose: () => void;
  /// Called with the file the user chose.
  onInstall: (project: Project, versionId: string) => void;
}) {
  const [showAll, setShowAll] = useState(false);
  const [chosen, setChosen] = useState<string | null>(null);

  // Every project opens on its own newest suitable file.
  useEffect(() => {
    setChosen(null);
    setShowAll(false);
  }, [project?.id]);

  const detail = useQuery({
    queryKey: ["mod-project", project?.source, project?.id],
    queryFn: () => ipc.modsProject(project!.source, project!.id),
    enabled: project !== null,
  });

  const versions = useQuery({
    queryKey: ["mod-versions", instance.id, project?.source, project?.id, showAll],
    queryFn: () => ipc.modsVersions(instance.id, project!.source, project!.id, !showAll),
    enabled: project !== null,
  });

  const full = detail.data ?? project;
  const listed = newestFirst(versions.data ?? []);
  const selected =
    listed.find((version) => version.id === chosen) ??
    newestCompatible(listed, loader, instance.mcVersion) ??
    listed[0] ??
    null;

  return (
    <Dialog open={project !== null} onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="max-w-2xl">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-3">
            <ProjectIcon url={full?.iconUrl ?? null} title={full?.title ?? ""} />
            <span className="min-w-0">
              <span className="block truncate">{full?.title}</span>
              <span className="block text-xs font-normal text-muted-foreground">
                {full?.author ?? "unknown author"} · {compactCount(full?.downloads ?? null)}{" "}
                downloads
                {full?.updated ? ` · updated ${relativeTime(full.updated)}` : ""}
              </span>
            </span>
          </DialogTitle>
          <DialogDescription className="sr-only">
            Project details and the version to install
          </DialogDescription>
        </DialogHeader>

        <div className="flex flex-wrap items-center gap-2 text-xs">
          {full?.license ? (
            <span className="flex items-center gap-1 text-muted-foreground">
              <Scale className="size-3.5" aria-hidden /> {full.license}
            </span>
          ) : null}
          {full?.pageUrl ? (
            <Button size="sm" variant="ghost" onClick={() => void openExternal(full.pageUrl!)}>
              <ExternalLink /> Project page
            </Button>
          ) : null}
          {full?.sourceUrl ? (
            <Button size="sm" variant="ghost" onClick={() => void openExternal(full.sourceUrl!)}>
              <Code2 /> Source
            </Button>
          ) : null}
          {full?.issuesUrl ? (
            <Button size="sm" variant="ghost" onClick={() => void openExternal(full.issuesUrl!)}>
              <Bug /> Issues
            </Button>
          ) : null}
        </div>

        <p className="max-h-40 overflow-y-auto whitespace-pre-wrap text-xs text-muted-foreground">
          {full?.body?.trim() || full?.description}
        </p>

        <div className="grid gap-1.5">
          <div className="flex items-center justify-between gap-2">
            <Label htmlFor="mod-version">Version</Label>
            <label className="flex items-center gap-1.5 text-xs text-muted-foreground">
              <input
                type="checkbox"
                className="size-3.5 accent-primary"
                checked={showAll}
                onChange={(event) => setShowAll(event.target.checked)}
              />
              Show versions that do not fit
            </label>
          </div>

          <Select
            id="mod-version"
            value={selected?.id ?? ""}
            onChange={(event) => setChosen(event.target.value)}
          >
            {listed.map((version) => {
              const reason = mismatchReason(version, loader, instance.mcVersion);
              return (
                <option
                  key={version.id}
                  value={version.id}
                  // Listed, never silently chosen: picking one deliberately is
                  // the point of the toggle above.
                  disabled={version.files.length === 0}
                >
                  {versionLabel(version)}
                  {version.id === installedVersionId ? " — installed" : ""}
                  {reason ? ` — ${reason}` : ""}
                  {version.files.length === 0 ? " — no downloadable file" : ""}
                </option>
              );
            })}
          </Select>

          {versions.isFetching ? (
            <p className="text-xs text-muted-foreground">Reading versions…</p>
          ) : listed.length === 0 ? (
            <p className="text-xs text-muted-foreground">
              Nothing published for {instance.mcVersion}. Tick the box to see every version.
            </p>
          ) : null}
        </div>

        {selected ? <Dependencies version={selected} /> : null}
        {versions.error ? <ErrorNotice error={versions.error} /> : null}

        <DialogFooter>
          <span className="mr-auto text-xs text-muted-foreground">
            {selected && mismatchReason(selected, loader, instance.mcVersion)
              ? `This file is ${mismatchReason(selected, loader, instance.mcVersion)} — install it only if you know why.`
              : selected?.published
                ? `Published ${publishedLabel(selected.published)}`
                : ""}
          </span>
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button
            disabled={!selected || selected.files.length === 0 || !(full?.downloadable ?? true)}
            onClick={() => full && selected && onInstall(full, selected.id)}
          >
            <Package /> {installLabel(selected, installedVersionId)}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

/**
 * What the selected version pulls in.
 *
 * Listed per version because they differ between them: a newer file often
 * needs a newer library, and choosing an older one changes the answer.
 */
function Dependencies({ version }: { version: SourceVersion }) {
  const required = version.dependencies.filter((dependency) => dependency.kind === "required");
  const optional = version.dependencies.filter((dependency) => dependency.kind === "optional");

  if (required.length === 0 && optional.length === 0) {
    return <p className="text-xs text-muted-foreground">This version needs nothing else.</p>;
  }

  return (
    <div className="grid gap-1 text-xs">
      {required.length > 0 ? (
        <p>
          <span className="font-medium">Requires: </span>
          <span className="text-muted-foreground">
            {required.map((dependency) => dependency.projectId ?? dependency.versionId).join(", ")}
          </span>
        </p>
      ) : null}
      {optional.length > 0 ? (
        <p>
          <span className="font-medium">Optional: </span>
          <span className="text-muted-foreground">
            {optional.map((dependency) => dependency.projectId ?? dependency.versionId).join(", ")}
          </span>
        </p>
      ) : null}
      <p className="text-muted-foreground">
        Required dependencies are resolved and confirmed before anything downloads.
      </p>
    </div>
  );
}

function ProjectIcon({ url, title }: { url: string | null; title: string }) {
  const cached = useQuery({
    queryKey: ["mod-icon", url],
    queryFn: () => ipc.modsIcon(url),
    enabled: url !== null,
    staleTime: Infinity,
  });

  if (!cached.data) {
    return (
      <span
        aria-hidden
        className="flex size-10 shrink-0 items-center justify-center rounded-md bg-muted text-sm font-medium text-muted-foreground"
      >
        {title.slice(0, 1).toUpperCase()}
      </span>
    );
  }
  return <img src={convertFileSrc(cached.data)} alt="" className="size-10 shrink-0 rounded-md" />;
}

/** Badge for a card: what the newest suitable file is. */
export function NewestBadge({ version }: { version: SourceVersion | null }) {
  if (!version) return null;
  return <Badge>{version.versionNumber || version.name}</Badge>;
}
