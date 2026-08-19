import { useQuery } from "@tanstack/react-query";
import { convertFileSrc } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import { ChevronLeft, ChevronRight, ExternalLink, Package, Search } from "lucide-react";
import { useEffect, useState } from "react";

import { Button } from "@/components/ui/button";
import { ErrorNotice } from "@/components/ui/ErrorNotice";
import { Input, Select } from "@/components/ui/input";
import { Badge } from "@/components/ui/misc";
import { ipc } from "@/lib/ipc";
import type {
  ContentType,
  ContentTypeOption,
  InstanceView,
  Project,
  SortBy,
  SourceId,
} from "@/lib/types";

import {
  CONTENT_TYPE_LABEL,
  PAGE_SIZE,
  SORT_OPTIONS,
  blockedReason,
  compactCount,
  pageOf,
  relativeTime,
} from "./browser";

/**
 * The browser: a grid of cards over whichever source, kind of content, sort and
 * category the user picked.
 *
 * Filtered to this instance's loader and Minecraft version by default, because
 * that is what can actually be installed — with a toggle for the times someone
 * wants to see everything.
 */
export function ModBrowser({
  instance,
  onInstall,
  onOpen,
}: {
  instance: InstanceView;
  /// The card's shortcut: the newest file that fits.
  onInstall: (project: Project) => void;
  /// Opening the card, where a particular version can be chosen.
  onOpen: (project: Project) => void;
}) {
  const [source, setSource] = useState<SourceId>("modrinth");
  const [text, setText] = useState("");
  const [submitted, setSubmitted] = useState("");
  const [contentType, setContentType] = useState<ContentType>("mod");
  const [sort, setSort] = useState<SortBy>("relevance");
  const [category, setCategory] = useState("");
  const [filterToInstance, setFilterToInstance] = useState(true);
  const [offset, setOffset] = useState(0);

  const sources = useQuery({ queryKey: ["mod-sources"], queryFn: () => ipc.modsSources() });
  const kinds = useQuery({
    queryKey: ["content-types", instance.id],
    queryFn: () => ipc.modsContentTypes(instance.id),
  });

  // The first kind the instance actually supports, so a Paper server opens on
  // plugins rather than on mods it cannot load.
  useEffect(() => {
    const first = kinds.data?.find((kind) => kind.installable);
    if (first) setContentType(first.contentType);
  }, [kinds.data]);

  const categories = useQuery({
    queryKey: ["mod-categories", source, contentType],
    queryFn: () => ipc.modsCategories(source, contentType),
    enabled: sources.data?.find((entry) => entry.id === source)?.configured ?? false,
  });

  const selected = sources.data?.find((entry) => entry.id === source);
  const page = useQuery({
    queryKey: [
      "mod-search",
      instance.id,
      source,
      submitted,
      contentType,
      sort,
      category,
      filterToInstance,
      offset,
    ],
    queryFn: () =>
      ipc.modsSearch({
        id: instance.id,
        source,
        text: submitted,
        contentType,
        sort,
        categories: category ? [category] : [],
        filterToInstance,
        limit: PAGE_SIZE,
        offset,
      }),
    enabled: selected?.configured ?? false,
  });

  // Any change of what is being asked for starts again at page one.
  useEffect(() => setOffset(0), [source, submitted, contentType, sort, category, filterToInstance]);

  const kindOf = (value: ContentType): ContentTypeOption | undefined =>
    kinds.data?.find((kind) => kind.contentType === value);
  const installable = kindOf(contentType)?.installable ?? false;
  const paging = pageOf(offset, page.data?.limit ?? PAGE_SIZE, page.data?.total ?? null);

  return (
    <section className="flex min-h-0 flex-col gap-3">
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
            placeholder={`Search ${selected?.name ?? ""} for ${CONTENT_TYPE_LABEL[
              contentType
            ].toLowerCase()}`}
            aria-label="Search"
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
          aria-label="Content type"
          value={contentType}
          onChange={(event) => setContentType(event.target.value as ContentType)}
        >
          {(kinds.data ?? []).map((kind) => (
            <option key={kind.contentType} value={kind.contentType}>
              {CONTENT_TYPE_LABEL[kind.contentType]}
              {kind.clientOnly ? " — client only" : ""}
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

        <Select
          className="h-8 w-auto text-xs"
          aria-label="Category"
          value={category}
          onChange={(event) => setCategory(event.target.value)}
        >
          <option value="">All categories</option>
          {(categories.data ?? []).map((entry) => (
            <option key={entry.id} value={entry.id}>
              {entry.name}
            </option>
          ))}
        </Select>

        <label className="flex items-center gap-1.5 text-xs text-muted-foreground">
          <input
            type="checkbox"
            className="size-3.5 accent-primary"
            checked={filterToInstance}
            onChange={(event) => setFilterToInstance(event.target.checked)}
          />
          Only what fits {instance.mcVersion}
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
        ) : page.data && page.data.projects.length === 0 ? (
          <p className="text-xs text-muted-foreground">
            Nothing matched
            {filterToInstance ? ` for ${instance.mcVersion}. Try turning the filter off.` : "."}
          </p>
        ) : (
          <ul className="grid gap-3 sm:grid-cols-2 xl:grid-cols-3">
            {(page.data?.projects ?? []).map((project) => (
              <ProjectCard
                key={`${project.source}-${project.id}`}
                project={project}
                installable={installable}
                onInstall={() => onInstall(project)}
                onOpen={() => onOpen(project)}
              />
            ))}
          </ul>
        )}
      </div>

      {page.data && page.data.projects.length > 0 ? (
        <nav className="flex items-center justify-between gap-2 text-xs" aria-label="Pages">
          <span className="text-muted-foreground">
            {page.data.total === null
              ? `Page ${paging.current}`
              : `Page ${paging.current} of ${paging.pages} · ${page.data.total} results`}
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
    </section>
  );
}

function ProjectCard({
  project,
  installable,
  onInstall,
  onOpen,
}: {
  project: Project;
  installable: boolean;
  onInstall: () => void;
  onOpen: () => void;
}) {
  const blocked = blockedReason(project, installable);

  return (
    <li className="flex flex-col gap-2 rounded-lg border border-border p-3">
      {/* The card itself opens the detail panel; the buttons below are the
          shortcuts. A button rather than a click handler on the li, so it is
          reachable by keyboard like everything else. */}
      <button
        type="button"
        className="flex items-start gap-3 text-left focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring"
        onClick={onOpen}
        aria-label={`Open ${project.title}`}
      >
        <ProjectIcon url={project.iconUrl} title={project.title} />
        <span className="min-w-0 flex-1">
          <span className="block truncate text-sm font-medium" title={project.title}>
            {project.title}
          </span>
          <span className="block truncate text-xs text-muted-foreground">
            {project.author ?? "unknown author"}
          </span>
        </span>
      </button>

      <p className="line-clamp-3 text-xs text-muted-foreground">{project.description}</p>

      <p className="flex flex-wrap items-center gap-2 text-[11px] text-muted-foreground">
        <span>{compactCount(project.downloads)} downloads</span>
        {project.updated ? (
          <span title={project.updated}>updated {relativeTime(project.updated)}</span>
        ) : null}
        {project.loaders.slice(0, 2).map((loader) => (
          <Badge key={loader}>{loader}</Badge>
        ))}
      </p>

      <div className="mt-auto flex items-center gap-2">
        <Button size="sm" disabled={blocked !== null} onClick={onInstall} title="The newest version that fits this server">
          <Package /> Install newest
        </Button>
        <Button size="sm" variant="outline" onClick={onOpen}>
          Versions…
        </Button>
        {project.pageUrl ? (
          <Button size="sm" variant="ghost" onClick={() => void openUrl(project.pageUrl!)}>
            <ExternalLink /> Page
          </Button>
        ) : null}
      </div>

      {blocked ? <p className="text-[11px] text-muted-foreground">{blocked}</p> : null}
    </li>
  );
}

/**
 * The icon, from the on-disk cache.
 *
 * Fetched once per URL by the backend and read from a file after that, so
 * scrolling the grid does not re-download anything. A project without one gets
 * its initial rather than a broken image.
 */
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

  return (
    <img
      src={convertFileSrc(cached.data)}
      alt=""
      className="size-10 shrink-0 rounded-md object-cover"
      loading="lazy"
    />
  );
}
