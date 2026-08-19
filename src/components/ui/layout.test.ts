import { readFileSync, readdirSync, statSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

/**
 * Layout rules that keep content reachable, checked across the whole UI.
 *
 * Every scrolling bug in this app has been the same shape — a box that cannot
 * shrink, or one that clips without scrolling — and each was found by a person
 * noticing something cut off. These read the source instead.
 */
function tsxFiles(dir = "src"): string[] {
  return readdirSync(dir).flatMap((entry) => {
    const path = join(dir, entry);
    if (statSync(path).isDirectory()) return tsxFiles(path);
    return path.endsWith(".tsx") && !path.endsWith(".test.tsx") ? [path] : [];
  });
}

/** Every className string literal in a file, with its line number. */
function classNames(source: string): { line: number; value: string }[] {
  const out: { line: number; value: string }[] = [];
  source.split("\n").forEach((text, index) => {
    for (const match of text.matchAll(/className=(?:"([^"]*)"|\{`([^`]*)`\}|\{cn\(\s*"([^"]*)")/g)) {
      out.push({ line: index + 1, value: match[1] ?? match[2] ?? match[3] ?? "" });
    }
  });
  return out;
}

const files = tsxFiles();

describe("scrolling containers", () => {
  it("caps a height only where something can scroll", () => {
    // A `max-h-*` with no overflow rule is a box that silently swallows
    // whatever does not fit — the install dialog's summary line, for instance.
    const offenders: string[] = [];

    for (const file of files) {
      for (const { line, value } of classNames(readFileSync(file, "utf8"))) {
        const capped = /\bmax-h-/.test(value) && !/\bmax-h-full\b/.test(value);
        const scrolls = /\boverflow(-[xy])?-(auto|scroll)\b/.test(value);
        // A capped box that is itself the scroll container's child and only
        // ever holds one line is fine, but those spell it out with truncate.
        const clipsOnPurpose = /\btruncate\b|\bline-clamp-/.test(value);
        if (capped && !scrolls && !clipsOnPurpose) {
          offenders.push(`${file}:${line} — ${value}`);
        }
      }
    }

    expect(offenders).toEqual([]);
  });

  it("lets a flex child shrink when it is meant to scroll", () => {
    // `flex-1` keeps `min-height: auto`, so a scrollable child grows past its
    // parent instead of scrolling inside it — and `body` has overflow hidden,
    // which makes the overflow unreachable rather than merely ugly.
    const offenders: string[] = [];

    for (const file of files) {
      for (const { line, value } of classNames(readFileSync(file, "utf8"))) {
        const grows = /\bflex-1\b/.test(value);
        const scrollsVertically = /\boverflow-y-(auto|scroll)\b/.test(value);
        const canShrink = /\bmin-h-0\b/.test(value) || /\bh-full\b/.test(value);
        if (grows && scrollsVertically && !canShrink) {
          offenders.push(`${file}:${line} — ${value}`);
        }
      }
    }

    expect(offenders).toEqual([]);
  });
});

describe("dialogs", () => {
  const dialog = readFileSync("src/components/ui/dialog.tsx", "utf8");

  it("caps the dialog and lets it scroll", () => {
    expect(dialog).toMatch(/max-h-\[90vh\]/);
    expect(dialog).toMatch(/overflow-y-auto/);
    // Reaching the end of a dialog must not start scrolling what is behind it.
    expect(dialog).toMatch(/overscroll-contain/);
  });

  it("never shortens the scroll area under a sticky footer", () => {
    // A negative bottom margin on the sticky footer removes exactly that much
    // from the scroll height, and the last line of content ends up underneath
    // it with nowhere left to scroll. Measured: 8px of "1 file to download,
    // 2.4 MB" hidden at 1000x700.
    const footer = dialog.slice(dialog.indexOf("DialogFooter"));
    expect(footer).toMatch(/sticky bottom-0/);
    expect(footer).not.toMatch(/-mb-\d/);
    // The footer carries the padding the container gives up, so the bottom
    // edge still looks flush.
    expect(dialog).toMatch(/pb-0/);
    expect(footer).toMatch(/pb-6/);
  });
});

describe("external links", () => {
  it("opens every address through the one helper", () => {
    // `void openUrl(...)` threw the promise away, so a rejected call — which is
    // what a missing URL scope produces — looked like a dead button.
    const offenders = files.filter((file) => {
      const source = readFileSync(file, "utf8");
      return (
        source.includes("@tauri-apps/plugin-opener") &&
        source.includes("openUrl") &&
        !file.endsWith("external.ts")
      );
    });

    expect(offenders).toEqual([]);
  });

  it("never opens a URL by any other route", () => {
    const offenders: string[] = [];
    for (const file of files) {
      const source = readFileSync(file, "utf8");
      if (/window\.open\(/.test(source)) offenders.push(`${file} — window.open`);
      // An <a href> to an external site opens inside the webview, which has no
      // way back.
      if (/href="https?:\/\//.test(source)) offenders.push(`${file} — <a href>`);
    }
    expect(offenders).toEqual([]);
  });
});
