import { describe, expect, it } from "vitest";

import type { InstalledMod, ModView } from "@/lib/types";
import {
  contentLabel,
  displayName,
  displayVersion,
  hasUpdate,
  mismatchSummary,
  sortForDisplay,
} from "./modLabels";

function tracked(overrides: Partial<InstalledMod> = {}): InstalledMod {
  return {
    id: 1,
    fileName: "lithium.jar",
    displayName: "Lithium",
    version: "0.15.3",
    loader: "fabric",
    mcVersion: "1.21.4",
    source: "modrinth",
    projectId: "gvQqBUqZ",
    versionId: "u8pHPXJl",
    pageUrl: null,
    sizeBytes: 797555,
    enabled: true,
    pinned: false,
    updateVersionId: null,
    installedAt: "2026-08-18T12:00:00Z",
    ...overrides,
  };
}

function mod(overrides: Partial<ModView> = {}): ModView {
  return {
    fileName: "lithium.jar",
    enabled: true,
    sizeBytes: 797555,
    tracked: null,
    metadata: null,
    mismatch: null,
    requiredBy: [],
    ...overrides,
  };
}

describe("contentLabel", () => {
  it("follows the loader rather than the files", () => {
    expect(contentLabel("paper")).toBe("Plugins");
    expect(contentLabel("fabric")).toBe("Mods");
    expect(contentLabel("neo_forge")).toBe("Mods");
    expect(contentLabel(null)).toBe("Mods");
  });
});

describe("names and versions", () => {
  it("prefers the tracked title, then the jar's own, then the file name", () => {
    expect(displayName(mod({ tracked: tracked() }))).toBe("Lithium");
    expect(
      displayName(
        mod({
          metadata: {
            format: "fabric.mod.json",
            id: "lithium",
            name: "Lithium (jar)",
            version: "0.15.3",
            description: null,
            authors: [],
            loaders: ["fabric"],
            gameVersions: ["1.21.4"],
          },
        }),
      ),
    ).toBe("Lithium (jar)");
    expect(displayName(mod())).toBe("lithium.jar");
  });

  it("returns null when nothing declares a version", () => {
    expect(displayVersion(mod())).toBeNull();
    expect(displayVersion(mod({ tracked: tracked() }))).toBe("0.15.3");
  });
});

describe("mismatchSummary", () => {
  it("is null for a jar that suits the instance", () => {
    expect(mismatchSummary(mod())).toBeNull();
  });

  it("joins both warnings when a jar is wrong twice over", () => {
    const summary = mismatchSummary(
      mod({
        mismatch: {
          loader: "this jar declares forge but the instance runs fabric",
          gameVersion: "this jar declares Minecraft 1.20.1 but the instance runs 1.21.4",
        },
      }),
    );
    expect(summary).toContain("forge");
    expect(summary).toContain("1.20.1");
  });
});

describe("hasUpdate", () => {
  it("is false for untracked and pinned mods", () => {
    expect(hasUpdate(mod())).toBe(false);
    expect(hasUpdate(mod({ tracked: tracked({ updateVersionId: "newer", pinned: true }) }))).toBe(
      false,
    );
  });

  it("is true when a check found something newer", () => {
    expect(hasUpdate(mod({ tracked: tracked({ updateVersionId: "newer" }) }))).toBe(true);
  });
});

describe("sortForDisplay", () => {
  it("puts problems first, then updates, then disabled jars", () => {
    const ordered = sortForDisplay([
      mod({ fileName: "fine.jar" }),
      mod({ fileName: "disabled.jar", enabled: false }),
      mod({ fileName: "update.jar", tracked: tracked({ updateVersionId: "newer" }) }),
      mod({
        fileName: "wrong.jar",
        mismatch: { loader: "wrong loader", gameVersion: null },
      }),
    ]);

    expect(ordered.map((entry) => entry.fileName)).toEqual([
      "wrong.jar",
      "update.jar",
      "disabled.jar",
      "fine.jar",
    ]);
  });

  it("does not mutate the input", () => {
    const input = [mod({ fileName: "b.jar" }), mod({ fileName: "a.jar" })];
    sortForDisplay(input);
    expect(input[0].fileName).toBe("b.jar");
  });
});
