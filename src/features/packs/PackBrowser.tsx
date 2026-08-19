import { useMutation, useQuery } from "@tanstack/react-query";
import { convertFileSrc } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  Boxes,
  CheckCircle2,
  ChevronLeft,
  ChevronRight,
  ExternalLink,
  Search,
  XCircle,
} from "lucide-react";
import { useEffect, useState } from "react";
import { toast } from "sonner";

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
import { ErrorNotice } from "@/components/ui/ErrorNotice";
import { Input, Select } from "@/components/ui/input";
import { PAGE_SIZE, SORT_OPTIONS, compactCount, pageOf, relativeTime } from "@/features/mods/browser";
import { newestFirst, versionLabel } from "@/features/mods/versions";
import { onTaskDone, onTaskProgress } from "@/lib/events";
import { formatBytes } from "@/lib/format";
import { ipc } from "@/lib/ipc";
import { toastError } from "@/lib/toast";
import type { Project, SortBy, SourceId } from "@/lib/types";

/**
 * Modpacks, browsed on their own.
 *
 * Not a per-instance tab: installing a pack *creates* the server, with the
 * loader, Minecraft version and Java the pack needs. The one filter that
 * matters is whether a pack has a server build at all — most do not, and a
 * launcher that finds that out half way through an install is the thing this
 * screen exists to avoid.
 */
export function PackBrowser({ onInstalled }: { onInstalled: (instanceId: number) => void }) {
  const [source, setSource] = useState<SourceId>("modrinth");
  const [text, setText] = useState("");
  const [submitted, setSubmitted] = useState("");
  const [sort, setSort] = useState<SortBy>("popularity");
  const [mcVersion, setMcVersion] = useState("");
  const [serverOnly, setServerOnly] = useState(true);
  const [offset, setOffset] = useState(0);
  const [chosen, setChosen] = useState<Project | null>(null);

  const sources = useQuery({ queryKey: ["mod-sources"], queryFn: () => ipc.modsSources() });
  const selected = sources.data?.find((entry) => entry.id === source);

  const page = useQuery({
    queryKey: ["pack-search", source, submitted, sort, mcVersion, serverOnly, offset],
    queryFn: () =>
      ipc.packsSearch({
        source,
        text: submitted,
        sort,
        categories: [],
        gameVersions: mcVersion.trim() ? [mcVersion.trim()] : [],
        serverOnly,
        limit: PAGE_SIZE,
        offset,
      }),
    enabled: selected?.configured ?? false,
  });

  useEffect(() => setOffset(0), [source, submitted, sort, mcVersion, serverOnly]);
  const paging = pageOf(offset, page.data?.limit ?? PAGE_SIZE, page.data?.total ?? null);

  return (
    <section className="flex min-h-0 flex-1 flex-col gap-3 p-6">
      <header>
        <h2 className="flex items-center gap-2 text-lg font-semibold">
          <Boxes className="size-5" aria-hidden />
          Browse modpacks
        </h2>
        <p className="text-xs text-muted-foreground">
          Installing a pack creates a new server with the loader and Minecraft version it needs.
        </p>
      </header>

      <form
        className="flex flex-wrap items-center gap-2"
        onSubmit={(event) => {
          event.preventDefault();
          setSubmitted(text.trim());
        }}
      >
        <div className="relative min-w-48 flex-1">
          <Search
            className="pointer-events-none absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground"
            aria-hidden
          />
          <Input
            className="h-8 pl-8 text-xs"
            placeholder="Search modpacks"
            aria-label="Search modpacks"
            value={text}
            onChange={(event) => setText(event.target.value)}
          />
        </div>

        <Select
          className="h-8 w-auto text-xs"
          aria-label="Source"
          value={source}
          onChange={(event) => setSource(event.target.value as SourceId)}
        >
          {(sources.data ?? []).map((entry) => (
            <option key={entry.id} value={entry.id}>
              {entry.name}
              {entry.configured ? "" : " (needs a key)"}
            </option>
          ))}
        </Select>

        <Select
          className="h-8 w-auto text-xs"
          aria-label="Sort by"
          value={sort}
          onChange={(event) => setSort(event.target.value as SortBy)}
        >
          {SORT_OPTIONS.map((option) => (
            <option key={option.value} value={option.value}>
              {option.label}
            </option>
          ))}
        </Select>

        <Input
          className="h-8 w-32 text-xs"
          placeholder="Any version"
          aria-label="Minecraft version"
          value={mcVersion}
          onChange={(event) => setMcVersion(event.target.value)}
        />

        <label className="flex items-center gap-1.5 text-xs text-muted-foreground">
          <input
            type="checkbox"
            className="size-3.5 accent-primary"
            checked={serverOnly}
            onChange={(event) => setServerOnly(event.target.checked)}
          />
          Only packs with a server build
        </label>
      </form>

      {selected && !selected.configured ? (
        <div className="rounded-md border border-border bg-muted/40 p-3 text-xs">
          <p>{selected.needs}</p>
          {selected.setupUrl ? (
            <Button
              size="sm"
              variant="outline"
              className="mt-2"
              onClick={() => void openUrl(selected.setupUrl!)}
            >
              <ExternalLink /> Get a key
            </Button>
          ) : null}
        </div>
      ) : null}

      {page.error ? <ErrorNotice error={page.error} /> : null}

      <div className="min-h-0 flex-1 overflow-y-auto overscroll-contain pr-1">
        {page.isFetching ? (
          <p className="text-xs text-muted-foreground">Searching…</p>
        ) : page.data?.projects.length === 0 ? (
          <p className="text-xs text-muted-foreground">Nothing matched.</p>
        ) : (
          <ul className="grid gap-3 sm:grid-cols-2 xl:grid-cols-3">
            {(page.data?.projects ?? []).map((pack) => (
              <PackCard key={`${pack.source}-${pack.id}`} pack={pack} onOpen={() => setChosen(pack)} />
            ))}
          </ul>
        )}
      </div>

      {page.data && page.data.projects.length > 0 ? (
        <nav className="flex items-center justify-between gap-2 text-xs" aria-label="Pages">
          <span className="text-muted-foreground">
            {page.data.total === null
              ? `Page ${paging.current}`
              : `Page ${paging.current} of ${paging.pages} · ${page.data.total} packs`}
          </span>
          <span className="flex gap-2">
            <Button
              size="sm"
              variant="outline"
              disabled={!paging.hasPrevious}
              onClick={() => setOffset(Math.max(0, offset - PAGE_SIZE))}
            >
              <ChevronLeft /> Previous
            </Button>
            <Button
              size="sm"
              variant="outline"
              disabled={!paging.hasNext}
              onClick={() => setOffset(offset + PAGE_SIZE)}
            >
              Next <ChevronRight />
            </Button>
          </span>
        </nav>
      ) : null}

      <InstallPackDialog
        pack={chosen}
        onClose={() => setChosen(null)}
        onInstalled={(instanceId) => {
          setChosen(null);
          onInstalled(instanceId);
        }}
      />
    </section>
  );
}

function PackCard({ pack, onOpen }: { pack: Project; onOpen: () => void }) {
  const icon = useQuery({
    queryKey: ["mod-icon", pack.iconUrl],
    queryFn: () => ipc.modsIcon(pack.iconUrl),
    enabled: pack.iconUrl !== null,
    staleTime: Infinity,
  });

  return (
    <li className="flex flex-col gap-2 rounded-lg border border-border p-3">
      <button
        type="button"
        className="flex items-start gap-3 text-left focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring"
        onClick={onOpen}
        aria-label={`Open ${pack.title}`}
      >
        {icon.data ? (
          <img src={convertFileSrc(icon.data)} alt="" className="size-10 shrink-0 rounded-md" />
        ) : (
          <span
            aria-hidden
            className="flex size-10 shrink-0 items-center justify-center rounded-md bg-muted text-sm text-muted-foreground"
          >
            {pack.title.slice(0, 1).toUpperCase()}
          </span>
        )}
        <span className="min-w-0 flex-1">
          <span className="block truncate text-sm font-medium">{pack.title}</span>
          <span className="block truncate text-xs text-muted-foreground">
            {pack.author ?? "unknown author"}
          </span>
        </span>
      </button>

      <p className="line-clamp-3 text-xs text-muted-foreground">{pack.description}</p>

      <p className="mt-auto flex flex-wrap items-center gap-2 text-[11px] text-muted-foreground">
        <span>{compactCount(pack.downloads)} downloads</span>
        {pack.updated ? <span>updated {relativeTime(pack.updated)}</span> : null}
        {pack.serverSide === "unsupported" ? (
          <span className="flex items-center gap-1 text-destructive">
            <XCircle className="size-3" aria-hidden /> no server build
          </span>
        ) : null}
      </p>

      <Button size="sm" onClick={onOpen}>
        <Boxes /> Set up a server
      </Button>
    </li>
  );
}

/**
 * Choosing a version, checking it, and creating the server.
 *
 * The check downloads the pack and reads its index, because that is the only
 * real answer to "does this have a server build" — and the install button stays
 * disabled until it says yes.
 */
function InstallPackDialog({
  pack,
  onClose,
  onInstalled,
}: {
  pack: Project | null;
  onClose: () => void;
  onInstalled: (instanceId: number) => void;
}) {
  const [versionId, setVersionId] = useState<string>("");
  const [name, setName] = useState("");
  const [folder, setFolder] = useState("");
  const [ram, setRam] = useState<number | null>(null);
  const [busy, setBusy] = useState<string | null>(null);

  useEffect(() => {
    setVersionId("");
    setName(pack?.title ?? "");
    setFolder("");
    setRam(null);
    setBusy(null);
  }, [pack?.id, pack?.title]);

  const versions = useQuery({
    queryKey: ["pack-versions", pack?.source, pack?.id],
    queryFn: () => ipc.packVersions(pack!.source, pack!.id),
    enabled: pack !== null,
  });

  const listed = newestFirst(versions.data ?? []);
  const chosenVersion = listed.find((version) => version.id === versionId) ?? listed[0] ?? null;

  const detail = useQuery({
    queryKey: ["pack-detail", pack?.source, pack?.id, chosenVersion?.id],
    queryFn: () => ipc.packExamine(pack!.source, pack!.id, chosenVersion!.id),
    enabled: pack !== null && chosenVersion !== null,
    retry: false,
  });

  // The pack's own answer for RAM, once it is known.
  useEffect(() => {
    if (detail.data && ram === null) {
      setRam(detail.data.publishedRamMb ?? detail.data.suggestedRamMb);
    }
  }, [detail.data, ram]);

  useEffect(() => {
    const pending = [
      onTaskProgress((payload) => {
        if (payload.kind === "pack_install") setBusy(payload.message);
      }),
      onTaskDone((payload) => {
        if (payload.kind !== "pack_install") return;
        setBusy(null);
        if (payload.ok && payload.instanceId !== null) {
          toast.success("The pack is installed");
          onInstalled(payload.instanceId);
        } else if (!payload.ok) {
          toast.error(payload.error ?? "The pack could not be installed");
        }
      }),
    ];
    return () => pending.forEach((promise) => void promise.then((unlisten) => unlisten()));
  }, [onInstalled]);

  const install = useMutation({
    mutationFn: () =>
      ipc.packInstall({
        source: pack!.source,
        projectId: pack!.id,
        versionId: chosenVersion!.id,
        name: name.trim(),
        path: folder.trim(),
        maxRamMb: ram,
      }),
    onSuccess: () => setBusy("Starting…"),
    onError: (error: unknown) => toastError(error),
  });

  const ready = detail.data?.support === "yes";

  return (
    <Dialog open={pack !== null} onOpenChange={(open) => !open && onClose()}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Set up a server from {pack?.title}</DialogTitle>
          <DialogDescription>
            The pack decides the loader and the Minecraft version; you choose where it lives.
          </DialogDescription>
        </DialogHeader>

        <div className="grid gap-3">
          <div className="grid gap-1.5">
            <Label htmlFor="pack-version">Pack version</Label>
            <Select
              id="pack-version"
              value={chosenVersion?.id ?? ""}
              onChange={(event) => setVersionId(event.target.value)}
            >
              {listed.map((version) => (
                <option key={version.id} value={version.id}>
                  {versionLabel(version)}
                </option>
              ))}
            </Select>
          </div>

          {detail.isFetching ? (
            <p className="text-xs text-muted-foreground">
              Reading the pack to see whether it has a server build…
            </p>
          ) : detail.data ? (
            <div className="rounded-md border border-border p-3 text-xs">
              {ready ? (
                <p className="flex items-center gap-1.5">
                  <CheckCircle2 className="size-3.5 text-emerald-500" aria-hidden />
                  {detail.data.loader} {detail.data.mcVersion} · {detail.data.serverFiles} files for
                  the server ({formatBytes(detail.data.totalBytes)})
                  {detail.data.clientOnlyFiles > 0
                    ? ` · ${detail.data.clientOnlyFiles} client-only file(s) skipped`
                    : ""}
                </p>
              ) : (
                <p className="flex items-start gap-1.5 text-destructive">
                  <XCircle className="mt-0.5 size-3.5 shrink-0" aria-hidden />
                  {detail.data.reason ?? "This pack has no server build."}
                </p>
              )}
            </div>
          ) : null}

          {detail.error ? <ErrorNotice error={detail.error} /> : null}

          <div className="grid gap-1.5">
            <Label htmlFor="pack-name">Server name</Label>
            <Input
              id="pack-name"
              value={name}
              onChange={(event) => setName(event.target.value)}
            />
          </div>

          <div className="grid gap-1.5">
            <Label htmlFor="pack-folder">Folder</Label>
            <div className="flex gap-2">
              <Input
                id="pack-folder"
                placeholder="Choose an empty folder"
                value={folder}
                onChange={(event) => setFolder(event.target.value)}
              />
              <Button
                type="button"
                variant="outline"
                onClick={async () => {
                  const picked = await openDialog({ directory: true, title: "Choose a folder" });
                  if (typeof picked === "string") setFolder(picked);
                }}
              >
                Browse…
              </Button>
            </div>
          </div>

          <div className="grid gap-1.5">
            <Label htmlFor="pack-ram">Maximum RAM (MB)</Label>
            <Input
              id="pack-ram"
              type="number"
              min={1024}
              step={512}
              value={ram ?? ""}
              onChange={(event) => setRam(Number(event.target.value))}
            />
            <p className="text-xs text-muted-foreground">
              {detail.data?.publishedRamMb
                ? "This is what the pack asks for."
                : "A starting point for a pack this size; raise it if the server struggles."}
            </p>
          </div>

          {busy ? (
            <p className="rounded-md border border-border bg-muted/40 px-3 py-2 text-xs" role="status">
              {busy}
            </p>
          ) : null}
        </div>

        <DialogFooter>
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button
            disabled={!ready || busy !== null || !name.trim() || !folder.trim()}
            onClick={() => install.mutate()}
          >
            <Boxes /> Create the server
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
