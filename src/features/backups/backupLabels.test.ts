import { describe, expect, it } from "vitest";

import type { Backup, Schedule, SpaceCheck } from "@/lib/types";

import {
  intervalLabel,
  isSafetyCopy,
  kindLabel,
  scheduleSummary,
  sortForDisplay,
  spaceWarning,
} from "./backupLabels";

function backup(id: number, createdAt: string, kind = "manual"): Backup {
  return {
    id,
    instanceId: 1,
    path: `/backups/${id}.tar.zst`,
    format: "tar_zst",
    scope: "full",
    kind,
    label: null,
    sizeBytes: 1024,
    createdAt,
  };
}

function schedule(overrides: Partial<Schedule> = {}): Schedule {
  return {
    id: 1,
    instanceId: 1,
    cron: null,
    intervalMinutes: 360,
    scope: "full",
    format: "tar_zst",
    compressionLevel: null,
    keepCount: null,
    keepDays: null,
    enabled: true,
    restartAfter: false,
    skipIfIdle: false,
    lastRunAt: null,
    nextRunAt: null,
    ...overrides,
  };
}

describe("backup labels", () => {
  it("names the kinds the way a person would", () => {
    expect(kindLabel("manual")).toBe("Manual");
    expect(kindLabel("scheduled")).toBe("Scheduled");
    expect(kindLabel("pre_restore")).toBe("Before restore");
  });

  it("marks the automatic pre-restore copy so it is not mistaken for a real backup", () => {
    expect(isSafetyCopy(backup(1, "2026-08-18T12:00:00Z", "pre_restore"))).toBe(true);
    expect(isSafetyCopy(backup(2, "2026-08-18T12:00:00Z"))).toBe(false);
  });

  it("lists newest first", () => {
    const rows = sortForDisplay([
      backup(1, "2026-08-17T12:00:00Z"),
      backup(2, "2026-08-19T12:00:00Z"),
      backup(3, "2026-08-18T12:00:00Z"),
    ]);
    expect(rows.map((row) => row.id)).toEqual([2, 3, 1]);
  });
});

describe("schedule summaries", () => {
  it("says intervals in the units a person would use", () => {
    expect(intervalLabel(30)).toBe("every 30 minutes");
    expect(intervalLabel(60)).toBe("every hour");
    expect(intervalLabel(360)).toBe("every 6 hours");
    expect(intervalLabel(1440)).toBe("every day");
    expect(intervalLabel(2880)).toBe("every 2 days");
  });

  it("reads back everything the schedule does", () => {
    const summary = scheduleSummary(
      schedule({
        intervalMinutes: 720,
        scope: "worlds",
        format: "zip",
        keepCount: 5,
        keepDays: 14,
        skipIfIdle: true,
        restartAfter: true,
      }),
    );

    expect(summary).toContain("Worlds only");
    expect(summary).toContain("zip");
    expect(summary).toContain("every 12 hours");
    expect(summary).toContain("keep 5");
    expect(summary).toContain("keep 14 days");
    expect(summary).toContain("skip when idle");
    expect(summary).toContain("restart after");
  });

  it("describes a daily schedule by its time", () => {
    expect(scheduleSummary(schedule({ intervalMinutes: null, cron: "04:30" }))).toContain(
      "daily at 04:30",
    );
  });

  it("says so when neither cadence is set rather than showing a blank", () => {
    expect(scheduleSummary(schedule({ intervalMinutes: null, cron: null }))).toContain(
      "no cadence set",
    );
  });
});

describe("free space", () => {
  const check = (overrides: Partial<SpaceCheck>): SpaceCheck => ({
    estimate: { files: 10, bytes: 1024 },
    requiredBytes: 1229,
    freeBytes: 4096,
    sufficient: true,
    message: null,
    ...overrides,
  });

  it("stays quiet when there is room", () => {
    expect(spaceWarning(check({}))).toBeNull();
    expect(spaceWarning(undefined)).toBeNull();
  });

  it("passes the backend's shortfall through verbatim", () => {
    const warning = spaceWarning(
      check({ sufficient: false, message: "needs 1.2 GB, 400 MB free" }),
    );
    expect(warning).toBe("needs 1.2 GB, 400 MB free");
  });

  it("still warns when the backend gave no wording", () => {
    expect(spaceWarning(check({ sufficient: false }))).toContain("not enough free space");
  });
});
