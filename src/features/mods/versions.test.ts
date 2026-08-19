import { describe, expect, it } from "vitest";

import type { SourceVersion } from "@/lib/types";

import {
  acceptedLoaders,
  installLabel,
  mismatchReason,
  newestCompatible,
  newestFirst,
  versionLabel,
} from "./versions";

function version(overrides: Partial<SourceVersion> = {}): SourceVersion {
  return {
    source: "modrinth",
    id: "v1",
    projectId: "AANobbMI",
    name: "Sodium 0.6.13",
    versionNumber: "0.6.13",
    channel: "release",
    published: "2026-08-14T10:00:00Z",
    gameVersions: ["1.21.4"],
    loaders: ["fabric"],
    files: [
      {
        url: "https://cdn.modrinth.com/data/AANobbMI/versions/v1/sodium.jar",
        fileName: "sodium-0.6.13.jar",
        sha1: null,
        sha512: null,
        size: 421_888,
        primary: true,
      },
    ],
    dependencies: [],
    ...overrides,
  };
}

describe("what fits this instance", () => {
  it("accepts a version published for the instance's loader and version", () => {
    expect(mismatchReason(version(), "fabric", "1.21.4")).toBeNull();
  });

  it("says which loader a version is for when it is the wrong one", () => {
    const reason = mismatchReason(version({ loaders: ["forge"] }), "fabric", "1.21.4");
    expect(reason).toBe("Forge only");
  });

  it("says which Minecraft version a file is for", () => {
    const reason = mismatchReason(version({ gameVersions: ["1.20.1"] }), "fabric", "1.21.4");
    expect(reason).toBe("for 1.20.1");

    // A long list is trimmed rather than filling the dropdown.
    const many = mismatchReason(
      version({ gameVersions: ["1.19", "1.19.1", "1.19.2", "1.19.3"] }),
      "fabric",
      "1.21.4",
    );
    expect(many).toBe("for 1.19, 1.19.1, 1.19.2…");
  });

  it("lets NeoForge take Forge files, but not the other way round", () => {
    expect(acceptedLoaders("neoforge")).toContain("forge");
    expect(acceptedLoaders("forge")).not.toContain("neoforge");

    expect(mismatchReason(version({ loaders: ["forge"] }), "neoforge", "1.21.4")).toBeNull();
    expect(mismatchReason(version({ loaders: ["neoforge"] }), "forge", "1.21.4")).toBe(
      "Neoforge only",
    );
  });

  it("accepts a plugin published for any Bukkit-family loader", () => {
    for (const published of ["paper", "spigot", "bukkit", "folia"]) {
      expect(mismatchReason(version({ loaders: [published] }), "paper", "1.21.4")).toBeNull();
    }
  });

  it("does not object when a version says nothing about loaders or versions", () => {
    expect(mismatchReason(version({ loaders: [], gameVersions: [] }), "fabric", "1.21.4")).toBeNull();
  });
});

describe("ordering and the default choice", () => {
  it("sorts by publish date, newest first", () => {
    const sorted = newestFirst([
      version({ id: "old", published: "2025-01-01T00:00:00Z" }),
      version({ id: "new", published: "2026-08-14T00:00:00Z" }),
      version({ id: "middle", published: "2026-02-01T00:00:00Z" }),
    ]);
    expect(sorted.map((entry) => entry.id)).toEqual(["new", "middle", "old"]);
  });

  it("puts a version with no date last rather than first", () => {
    const sorted = newestFirst([
      version({ id: "undated", published: null }),
      version({ id: "dated", published: "2020-01-01T00:00:00Z" }),
    ]);
    expect(sorted.map((entry) => entry.id)).toEqual(["dated", "undated"]);
  });

  it("defaults to the newest file that actually fits", () => {
    const chosen = newestCompatible(
      [
        version({ id: "newer-but-wrong", published: "2026-08-18T00:00:00Z", gameVersions: ["1.21.5"] }),
        version({ id: "fits", published: "2026-08-01T00:00:00Z" }),
        version({ id: "older", published: "2025-01-01T00:00:00Z" }),
      ],
      "fabric",
      "1.21.4",
    );
    expect(chosen?.id).toBe("fits");
  });

  it("never defaults to a version with nothing to download", () => {
    // A CurseForge file whose author forbids API downloads.
    const chosen = newestCompatible(
      [
        version({ id: "restricted", published: "2026-08-18T00:00:00Z", files: [] }),
        version({ id: "fine", published: "2026-08-01T00:00:00Z" }),
      ],
      "fabric",
      "1.21.4",
    );
    expect(chosen?.id).toBe("fine");
  });

  it("says nothing fits when nothing does", () => {
    expect(newestCompatible([version({ loaders: ["forge"] })], "fabric", "1.21.4")).toBeNull();
    expect(newestCompatible([], "fabric", "1.21.4")).toBeNull();
  });
});

describe("what the dropdown and the button say", () => {
  it("describes a version by number, channel, versions, date and size", () => {
    const label = versionLabel(version({ channel: "beta" }));
    expect(label).toContain("0.6.13");
    expect(label).toContain("beta");
    expect(label).toContain("1.21.4");
    expect(label).toContain("2026");
    expect(label).toContain("412.0 KB");
  });

  it("names what the install button will do", () => {
    const selected = version({ versionNumber: "0.6.13" });

    expect(installLabel(selected, null)).toBe("Install 0.6.13");
    // Already on a different version: this is a switch, including downwards.
    expect(installLabel(selected, "v-other")).toBe("Switch to 0.6.13");
    // The same one: reinstalling is legitimate, and the button says so.
    expect(installLabel(selected, "v1")).toBe("Reinstall this version");
    expect(installLabel(null, null)).toBe("Install");
  });
});
