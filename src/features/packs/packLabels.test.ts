import { describe, expect, it } from "vitest";

import { formatBytes } from "@/lib/format";
import type { PackDetail, Project } from "@/lib/types";

/**
 * What the pack screen decides from the backend's answer. The suitability rule
 * lives in Rust — this pins the shape the UI branches on, so a card cannot end
 * up offering an install the backend would refuse.
 */
function detail(overrides: Partial<PackDetail> = {}): PackDetail {
  return {
    name: "Test Pack",
    mcVersion: "1.21.4",
    loader: "fabric",
    loaderVersion: "0.16.9",
    serverType: "fabric",
    support: "yes",
    reason: null,
    serverFiles: 120,
    clientOnlyFiles: 14,
    totalBytes: 184_549_376,
    publishedRamMb: null,
    suggestedRamMb: 6144,
    ...overrides,
  };
}

function pack(overrides: Partial<Project> = {}): Project {
  return {
    source: "modrinth",
    id: "1KVo5zza",
    slug: "adrenaline",
    title: "Adrenaline",
    description: "A kitchen-sink pack",
    author: "someone",
    downloads: 1_400_000,
    iconUrl: null,
    pageUrl: "https://modrinth.com/modpack/adrenaline",
    categories: [],
    loaders: ["fabric"],
    updated: "2026-07-01T00:00:00Z",
    contentType: "modpack",
    license: null,
    sourceUrl: null,
    issuesUrl: null,
    wikiUrl: null,
    body: null,
    serverSide: "required",
    downloadable: true,
    ...overrides,
  };
}

/** The install button is enabled only for a pack that really has a server build. */
function canInstall(value: PackDetail | null): boolean {
  return value?.support === "yes";
}

describe("server suitability", () => {
  it("allows an install only when the index says there is a server build", () => {
    expect(canInstall(detail())).toBe(true);
    expect(canInstall(null)).toBe(false);
  });

  it("refuses a pack whose files are all client-only, with the reason to show", () => {
    const clientOnly = detail({
      support: "no",
      serverFiles: 0,
      reason: "Every file in this pack is marked client-only, so there is no server build.",
    });

    expect(canInstall(clientOnly)).toBe(false);
    expect(clientOnly.reason).toContain("no server build");
  });

  it("refuses a loader this app cannot run, and names it", () => {
    const quilt = detail({
      support: "no",
      serverType: null,
      loader: "quilt",
      reason: "This pack is built for quilt, which this app cannot run as a server.",
    });

    expect(canInstall(quilt)).toBe(false);
    expect(quilt.reason).toContain("quilt");
  });

  it("does not treat an unknown answer as a yes", () => {
    // CurseForge says nothing about server support until the index is read.
    expect(canInstall(detail({ support: "unknown" }))).toBe(false);
  });

  it("marks a card the source has already ruled out", () => {
    expect(pack({ serverSide: "unsupported" }).serverSide).toBe("unsupported");
    expect(pack().serverSide).toBe("required");
    // CurseForge packs carry nothing, and are decided by their index.
    expect(pack({ source: "curse_forge", serverSide: null }).serverSide).toBeNull();
  });
});

describe("what the install form pre-fills", () => {
  it("prefers the RAM the pack asks for", () => {
    const asked = detail({ publishedRamMb: 8192, suggestedRamMb: 6144 });
    expect(asked.publishedRamMb ?? asked.suggestedRamMb).toBe(8192);
  });

  it("falls back to a suggestion sized for the pack", () => {
    const quiet = detail({ publishedRamMb: null, suggestedRamMb: 6144 });
    expect(quiet.publishedRamMb ?? quiet.suggestedRamMb).toBe(6144);
  });

  it("shows what will be installed in units a person reads", () => {
    expect(formatBytes(detail().totalBytes)).toBe("176.0 MB");
    expect(detail().clientOnlyFiles).toBe(14);
  });
});
