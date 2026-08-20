import { describe, expect, it } from "vitest";

import { filterVersions, releaseDate } from "./VersionTable";
import type { VersionEntry, VersionKind } from "@/lib/types";

function entry(id: string, kind: VersionKind, releaseTime: string | null): VersionEntry {
  return { id, kind, releaseTime, stable: kind === "release" };
}

// Newest first, as the backend sends them: a release, the pre-release that led
// to it, a snapshot, and an older release from the classic era.
const rows: VersionEntry[] = [
  entry("26.2", "release", "2026-06-17T10:00:00+00:00"),
  entry("26.2-pre1", "pre_release", "2026-06-03T10:00:00+00:00"),
  entry("26w20a", "snapshot", "2026-05-13T10:00:00+00:00"),
  entry("1.21.4", "release", "2024-12-03T10:00:00+00:00"),
];

describe("version filters", () => {
  it("shows releases only by default", () => {
    const kept = filterVersions(rows, new Set<VersionKind>(["release"]));
    expect(kept.map((row) => row.id)).toEqual(["26.2", "1.21.4"]);
  });

  it("keeps pre-releases separate from snapshots", () => {
    expect(filterVersions(rows, new Set<VersionKind>(["pre_release"])).map((row) => row.id)).toEqual(
      ["26.2-pre1"],
    );
    expect(filterVersions(rows, new Set<VersionKind>(["snapshot"])).map((row) => row.id)).toEqual([
      "26w20a",
    ]);
  });

  it("never reorders what the backend sent", () => {
    // Release chronology crosses the two numbering eras, and only Rust knows
    // it — a client-side sort would put 1.21.4 above 26.2.
    const kept = filterVersions(rows, new Set<VersionKind>(["release", "snapshot", "pre_release"]));
    expect(kept.map((row) => row.id)).toEqual(rows.map((row) => row.id));
  });
});

describe("release dates", () => {
  it("renders a date for versions the manifest lists", () => {
    expect(releaseDate("2024-12-03T10:00:00+00:00")).toMatch(/2024/);
  });

  it("says nothing rather than inventing a date", () => {
    expect(releaseDate(null)).toBe("—");
    expect(releaseDate("not a date")).toBe("—");
  });
});
