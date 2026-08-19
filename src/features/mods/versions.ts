// How a version reads in the picker, and why one does not fit this instance.
// Pure, so the wording and the compatibility rule are testable without a source.
import { formatBytes } from "@/lib/format";
import type { SourceVersion } from "@/lib/types";

/** Loaders this instance can load, given its own. */
export function acceptedLoaders(loader: string | null): string[] {
  switch (loader) {
    case "fabric":
      return ["fabric"];
    case "forge":
      return ["forge"];
    // NeoForge still loads a good deal of Forge content; the reverse is not
    // true, so the widening only goes one way.
    case "neoforge":
      return ["neoforge", "forge"];
    case "paper":
      return ["paper", "spigot", "bukkit", "folia"];
    default:
      return [];
  }
}

/**
 * Why this version does not suit the instance, or null when it does.
 *
 * A version listing everything is deliberately offered — sometimes an older
 * file is exactly what someone needs — so each entry says what is wrong with it
 * rather than being hidden.
 */
export function mismatchReason(
  version: SourceVersion,
  loader: string | null,
  mcVersion: string,
): string | null {
  const accepted = acceptedLoaders(loader);
  const loaderFits =
    version.loaders.length === 0 ||
    accepted.length === 0 ||
    version.loaders.some((published) => accepted.includes(published.toLowerCase()));

  if (!loaderFits) {
    const names = version.loaders.map(titleCase).join(", ");
    return `${names} only`;
  }

  const versionFits =
    version.gameVersions.length === 0 || version.gameVersions.includes(mcVersion);
  if (!versionFits) {
    const shown = version.gameVersions.slice(0, 3).join(", ");
    return `for ${shown}${version.gameVersions.length > 3 ? "…" : ""}`;
  }

  return null;
}

function titleCase(value: string): string {
  return value.charAt(0).toUpperCase() + value.slice(1);
}

/** "0.6.13 · release · 1.21.4 · 14 Aug 2026 · 412 KB" */
export function versionLabel(version: SourceVersion): string {
  const parts = [version.versionNumber || version.name, version.channel];

  if (version.gameVersions.length > 0) {
    parts.push(version.gameVersions.slice(0, 2).join(", "));
  }
  const published = publishedLabel(version.published);
  if (published) parts.push(published);

  const size = version.files.find((file) => file.primary)?.size ?? version.files[0]?.size ?? null;
  if (size !== null) parts.push(formatBytes(Number(size)));

  return parts.join(" · ");
}

export function publishedLabel(published: string | null): string {
  if (!published) return "";
  const at = new Date(published);
  if (Number.isNaN(at.getTime())) return "";
  return at.toLocaleDateString([], { day: "numeric", month: "short", year: "numeric" });
}

/**
 * Versions newest first, as the source dated them.
 *
 * The sources return their own order and it is not always chronological; the
 * publish date is, and a file with no date sorts last rather than to the top.
 */
export function newestFirst(versions: SourceVersion[]): SourceVersion[] {
  return [...versions].sort((left, right) => {
    const a = left.published ? Date.parse(left.published) : Number.NEGATIVE_INFINITY;
    const b = right.published ? Date.parse(right.published) : Number.NEGATIVE_INFINITY;
    return b - a;
  });
}

/** The newest version that suits the instance — what the card's Install does. */
export function newestCompatible(
  versions: SourceVersion[],
  loader: string | null,
  mcVersion: string,
): SourceVersion | null {
  return (
    newestFirst(versions).find(
      (version) =>
        mismatchReason(version, loader, mcVersion) === null && version.files.length > 0,
    ) ?? null
  );
}

/** What the install button says, so a click is never a surprise. */
export function installLabel(
  selected: SourceVersion | null,
  installedVersionId: string | null,
): string {
  if (!selected) return "Install";
  if (selected.id === installedVersionId) return "Reinstall this version";

  const number = selected.versionNumber || selected.name;
  return installedVersionId ? `Switch to ${number}` : `Install ${number}`;
}
