import { describe, expect, it } from "vitest";

import type { Sample } from "@/lib/types";

import { CHART_HEIGHT, CHART_WIDTH, areaPath, latest, linePath, timeLabels, toSeries } from "./chart";

function sample(ts: string, cpuPct: number, rssBytes = 0, players: number | null = null): Sample {
  return { ts, cpuPct, rssBytes, players };
}

describe("toSeries", () => {
  it("spaces points by time, not by sample index", () => {
    // Two samples a minute apart, then a nine-minute gap: the last point sits at
    // the right edge and the middle one a tenth of the way across, because the
    // server was not running in between.
    const series = toSeries(
      [
        sample("2026-08-18T12:00:00Z", 10),
        sample("2026-08-18T12:01:00Z", 20),
        sample("2026-08-18T12:10:00Z", 30),
      ],
      (entry) => entry.cpuPct,
    );

    expect(series.points).toHaveLength(3);
    expect(series.points[0].x).toBe(0);
    expect(series.points[1].x).toBeCloseTo(CHART_WIDTH * 0.1, 5);
    expect(series.points[2].x).toBe(CHART_WIDTH);
  });

  it("leaves headroom above the peak unless a ceiling is given", () => {
    const auto = toSeries([sample("2026-08-18T12:00:00Z", 50)], (entry) => entry.cpuPct);
    expect(auto.max).toBeCloseTo(55, 5);

    // With an explicit ceiling — the allocated heap — the peak is drawn against
    // that instead, which is what makes "half its heap" readable.
    const fixed = toSeries(
      [sample("2026-08-18T12:00:00Z", 0, 512)],
      (entry) => entry.rssBytes,
      { max: 1024 },
    );
    expect(fixed.max).toBe(1024);
    expect(fixed.points[0].y).toBeCloseTo(CHART_HEIGHT / 2, 5);
  });

  it("clamps a value above the ceiling to the top edge rather than off it", () => {
    const series = toSeries(
      [sample("2026-08-18T12:00:00Z", 0, 4096)],
      (entry) => entry.rssBytes,
      { max: 1024 },
    );
    expect(series.points[0].y).toBe(0);
  });

  it("skips samples with no value and unparsable timestamps", () => {
    const series = toSeries(
      [
        sample("2026-08-18T12:00:00Z", 0, 0, 3),
        sample("2026-08-18T12:01:00Z", 0, 0, null),
        sample("not a date", 0, 0, 5),
      ],
      (entry) => entry.players,
    );
    expect(series.points).toHaveLength(1);
  });

  it("returns nothing to draw for an empty window", () => {
    expect(toSeries([], (entry) => entry.cpuPct).points).toEqual([]);
  });
});

describe("paths", () => {
  it("draws a visible mark for a single sample", () => {
    const series = toSeries([sample("2026-08-18T12:00:00Z", 10)], (entry) => entry.cpuPct);
    expect(linePath(series.points)).toMatch(/^M .* L /);
  });

  it("closes the area back to the baseline", () => {
    const series = toSeries(
      [sample("2026-08-18T12:00:00Z", 10), sample("2026-08-18T12:01:00Z", 20)],
      (entry) => entry.cpuPct,
    );
    const path = areaPath(series.points);
    expect(path.endsWith("Z")).toBe(true);
    expect(path).toContain(`${CHART_HEIGHT}`);
  });

  it("produces no path at all when there is nothing to draw", () => {
    expect(linePath([])).toBe("");
    expect(areaPath([])).toBe("");
  });
});

describe("labels", () => {
  it("labels the first and last sample and reports the newest", () => {
    const rows = [sample("2026-08-18T12:00:00Z", 10), sample("2026-08-18T13:00:00Z", 20)];
    const labels = timeLabels(rows);

    expect(labels.start).not.toBe("");
    expect(labels.end).not.toBe("");
    expect(latest(rows)?.cpuPct).toBe(20);
  });

  it("has nothing to say about an empty window", () => {
    expect(timeLabels([])).toEqual({ start: "", end: "" });
    expect(latest([])).toBeNull();
  });
});
