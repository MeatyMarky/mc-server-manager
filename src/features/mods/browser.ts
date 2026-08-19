// The browser's own vocabulary: what the dropdowns say, and what a card shows.
// Pure, so the wording and the number formatting are testable.
import type { ContentType, Project, SortBy, SourceId } from "@/lib/types";

export const SORT_OPTIONS: { value: SortBy; label: string }[] = [
  { value: "relevance", label: "Relevance" },
  { value: "popularity", label: "Popularity" },
  { value: "downloads", label: "Downloads" },
  { value: "recently_updated", label: "Recently updated" },
  { value: "newest", label: "Newest" },
];

export const CONTENT_TYPE_LABEL: Record<ContentType, string> = {
  mod: "Mods",
  plugin: "Plugins",
  modpack: "Modpacks",
  data_pack: "Data packs",
  resource_pack: "Resource packs",
  shader: "Shaders",
};

export const SOURCE_LABEL: Record<SourceId, string> = {
  modrinth: "Modrinth",
  curse_forge: "CurseForge",
  local: "Local file",
};

/** Page size for the grid. */
export const PAGE_SIZE = 20;

/** 412000000 as "412M", which is what fits on a card. */
export function compactCount(value: number | null): string {
  if (value === null || !Number.isFinite(value) || value < 0) return "—";
  if (value < 1_000) return String(value);
  if (value < 1_000_000) return `${(value / 1_000).toFixed(value < 10_000 ? 1 : 0)}K`;
  if (value < 1_000_000_000) return `${(value / 1_000_000).toFixed(value < 10_000_000 ? 1 : 0)}M`;
  return `${(value / 1_000_000_000).toFixed(1)}B`;
}

/** "3 days ago" for a card; the exact stamp goes in the title attribute. */
export function relativeTime(iso: string | null, now = Date.now()): string {
  if (!iso) return "";
  const at = new Date(iso).getTime();
  if (Number.isNaN(at)) return "";

  const days = Math.floor((now - at) / 86_400_000);
  if (days < 1) return "today";
  if (days === 1) return "yesterday";
  if (days < 30) return `${days} days ago`;
  const months = Math.floor(days / 30);
  if (months < 12) return `${months} month${months === 1 ? "" : "s"} ago`;
  const years = Math.floor(days / 365);
  return `${years} year${years === 1 ? "" : "s"} ago`;
}

/** Which page the current offset is on, and how many there are. */
export function pageOf(offset: number, limit: number, total: number | null) {
  const size = Math.max(1, limit);
  const current = Math.floor(offset / size) + 1;
  const pages = total === null ? null : Math.max(1, Math.ceil(total / size));
  return { current, pages, hasPrevious: offset > 0, hasNext: pages === null ? true : current < pages };
}

/**
 * Why a project cannot be installed here, or null when it can.
 *
 * A CurseForge author may forbid third-party downloads: the file exists, the
 * API will not serve it, and the honest answer is a link to the page rather
 * than a failure three clicks later.
 */
export function blockedReason(project: Project, installable: boolean): string | null {
  if (!project.downloadable) {
    return "The author does not allow downloads through the API — get it from the project page.";
  }
  if (!installable) {
    return "This kind of content is for the client; a server never loads it.";
  }
  return null;
}
