import { describe, expect, it } from "vitest";

import type { Project } from "@/lib/types";

import {
  CONTENT_TYPE_LABEL,
  PAGE_SIZE,
  SORT_OPTIONS,
  blockedReason,
  compactCount,
  pageOf,
  relativeTime,
} from "./browser";

function project(overrides: Partial<Project> = {}): Project {
  return {
    source: "modrinth",
    id: "AANobbMI",
    slug: "sodium",
    title: "Sodium",
    description: "A rendering engine",
    author: "jellysquid3",
    downloads: 412_000_000,
    iconUrl: "https://cdn.modrinth.com/data/AANobbMI/icon.png",
    pageUrl: "https://modrinth.com/mod/sodium",
    categories: ["optimization"],
    loaders: ["fabric"],
    updated: "2026-08-01T10:00:00Z",
    contentType: "mod",
    license: "MIT",
    sourceUrl: null,
    issuesUrl: null,
    wikiUrl: null,
    body: null,
    downloadable: true,
    ...overrides,
  };
}

describe("card figures", () => {
  it("shortens download counts to what fits on a card", () => {
    expect(compactCount(0)).toBe("0");
    expect(compactCount(942)).toBe("942");
    expect(compactCount(1_500)).toBe("1.5K");
    expect(compactCount(48_000)).toBe("48K");
    expect(compactCount(1_200_000)).toBe("1.2M");
    expect(compactCount(412_000_000)).toBe("412M");
    expect(compactCount(2_400_000_000)).toBe("2.4B");
  });

  it("says nothing rather than zero when the count is unknown", () => {
    expect(compactCount(null)).toBe("—");
  });

  it("dates a project in words", () => {
    const now = Date.parse("2026-08-19T12:00:00Z");
    expect(relativeTime("2026-08-19T09:00:00Z", now)).toBe("today");
    expect(relativeTime("2026-08-18T09:00:00Z", now)).toBe("yesterday");
    expect(relativeTime("2026-08-04T12:00:00Z", now)).toBe("15 days ago");
    expect(relativeTime("2026-06-04T12:00:00Z", now)).toBe("2 months ago");
    expect(relativeTime("2024-06-04T12:00:00Z", now)).toBe("2 years ago");

    // A project with no publish date shows nothing, not "Invalid Date".
    expect(relativeTime(null, now)).toBe("");
    expect(relativeTime("whenever", now)).toBe("");
  });
});

describe("pagination", () => {
  it("counts pages from the offset and the total", () => {
    const first = pageOf(0, PAGE_SIZE, 95);
    expect(first.current).toBe(1);
    expect(first.pages).toBe(5);
    expect(first.hasPrevious).toBe(false);
    expect(first.hasNext).toBe(true);

    const last = pageOf(80, PAGE_SIZE, 95);
    expect(last.current).toBe(5);
    expect(last.hasNext).toBe(false);
    expect(last.hasPrevious).toBe(true);
  });

  it("keeps going when the source does not say how many there are", () => {
    // CurseForge can answer without a total; the pager must not conclude that
    // there is nothing more.
    const page = pageOf(40, PAGE_SIZE, null);
    expect(page.current).toBe(3);
    expect(page.pages).toBeNull();
    expect(page.hasNext).toBe(true);
  });

  it("treats an empty result as one page", () => {
    expect(pageOf(0, PAGE_SIZE, 0).pages).toBe(1);
  });
});

describe("what a card may install", () => {
  it("offers an install for content this server loads", () => {
    expect(blockedReason(project(), true)).toBeNull();
  });

  it("explains a CurseForge project that forbids API downloads", () => {
    const reason = blockedReason(project({ source: "curse_forge", downloadable: false }), true);
    expect(reason).toContain("does not allow downloads");
    expect(reason).toContain("project page");
  });

  it("explains client-only content rather than letting the install fail", () => {
    const reason = blockedReason(project({ contentType: "shader" }), false);
    expect(reason).toContain("for the client");
  });

  it("puts the download restriction first when both apply", () => {
    // Nothing can be installed either way; the more specific reason is the one
    // worth showing.
    const reason = blockedReason(project({ downloadable: false }), false);
    expect(reason).toContain("does not allow downloads");
  });
});

describe("dropdown vocabulary", () => {
  it("offers every sort the sources can do", () => {
    expect(SORT_OPTIONS.map((option) => option.value)).toEqual([
      "relevance",
      "popularity",
      "downloads",
      "recently_updated",
      "newest",
    ]);
  });

  it("names every content type", () => {
    expect(CONTENT_TYPE_LABEL.mod).toBe("Mods");
    expect(CONTENT_TYPE_LABEL.plugin).toBe("Plugins");
    expect(CONTENT_TYPE_LABEL.data_pack).toBe("Data packs");
    expect(CONTENT_TYPE_LABEL.resource_pack).toBe("Resource packs");
    expect(CONTENT_TYPE_LABEL.shader).toBe("Shaders");
  });
});
