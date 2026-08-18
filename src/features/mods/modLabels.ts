// Presentation helpers for the mods tab. Pure, so they can be tested without a
// backend; every decision that matters (what is installable, where it goes) is
// made in Rust and only rendered here.
import type { Loader, ModView } from "@/lib/types";

/** What the tab calls its content, which follows the loader, not the files. */
export function contentLabel(loader: Loader | null): string {
  return loader === "paper" ? "Plugins" : "Mods";
}

/** The name to show for a jar: tracked title, then jar metadata, then the file. */
export function displayName(mod: ModView): string {
  return mod.tracked?.displayName ?? mod.metadata?.name ?? mod.fileName;
}

/** The version to show, if anything knows one. */
export function displayVersion(mod: ModView): string | null {
  return mod.tracked?.version ?? mod.metadata?.version ?? null;
}

/** Mismatch warnings as one line, or null when the jar suits the instance. */
export function mismatchSummary(mod: ModView): string | null {
  if (!mod.mismatch) return null;
  const parts = [mod.mismatch.loader, mod.mismatch.gameVersion].filter(Boolean);
  return parts.length > 0 ? parts.join(" · ") : null;
}

/** True when an update check found a newer version and the mod is not pinned. */
export function hasUpdate(mod: ModView): boolean {
  const tracked = mod.tracked;
  return Boolean(tracked && tracked.updateVersionId && !tracked.pinned);
}

/** Sorts what needs attention to the top, then keeps file order stable. */
export function sortForDisplay(mods: ModView[]): ModView[] {
  const weight = (mod: ModView) => {
    if (mismatchSummary(mod)) return 0;
    if (hasUpdate(mod)) return 1;
    if (!mod.enabled) return 2;
    return 3;
  };
  return [...mods].sort(
    (a, b) => weight(a) - weight(b) || a.fileName.localeCompare(b.fileName),
  );
}
