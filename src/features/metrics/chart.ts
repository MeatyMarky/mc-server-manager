// The maths behind the charts, kept out of the component so it can be tested
// without rendering anything.
import type { Sample } from "@/lib/types";

export type Point = { x: number; y: number };

export type Series = {
  /** Points in SVG coordinates, ready for a polyline. */
  points: Point[];
  /** The value the y axis tops out at, after headroom is added. */
  max: number;
};

export const CHART_WIDTH = 600;
export const CHART_HEIGHT = 140;

/**
 * Projects samples onto the chart box.
 *
 * The x axis is real time, not sample index: a server that was stopped for an
 * hour leaves a gap in the data, and drawing by index would quietly stretch the
 * remaining samples over the whole window as if it had been running all along.
 */
export function toSeries(
  samples: Sample[],
  value: (sample: Sample) => number | null,
  options: { min?: number; max?: number; width?: number; height?: number } = {},
): Series {
  const width = options.width ?? CHART_WIDTH;
  const height = options.height ?? CHART_HEIGHT;

  const usable = samples
    .map((sample) => ({ at: Date.parse(sample.ts), value: value(sample) }))
    .filter((entry): entry is { at: number; value: number } =>
      Number.isFinite(entry.at) && entry.value !== null && Number.isFinite(entry.value),
    );

  if (usable.length === 0) return { points: [], max: options.max ?? 1 };

  const first = usable[0].at;
  const last = usable[usable.length - 1].at;
  const span = Math.max(1, last - first);

  const highest = Math.max(...usable.map((entry) => entry.value), options.min ?? 0);
  // A fixed ceiling (the heap limit) wins; otherwise leave 10% headroom so the
  // peak is not drawn flush against the top edge.
  const max = options.max ?? Math.max(highest * 1.1, 1);

  return {
    points: usable.map((entry) => ({
      x: ((entry.at - first) / span) * width,
      y: height - Math.min(1, entry.value / max) * height,
    })),
    max,
  };
}

/** An SVG path for a line chart. */
export function linePath(points: Point[]): string {
  if (points.length === 0) return "";
  if (points.length === 1) {
    // One sample would otherwise draw nothing at all: give it a visible dash.
    const { x, y } = points[0];
    return `M ${x - 2} ${y} L ${x + 2} ${y}`;
  }
  return points
    .map((point, index) => `${index === 0 ? "M" : "L"} ${point.x.toFixed(1)} ${point.y.toFixed(1)}`)
    .join(" ");
}

/** The same line closed to the baseline, for the filled area under it. */
export function areaPath(points: Point[], height = CHART_HEIGHT): string {
  if (points.length === 0) return "";
  const line = linePath(points);
  const first = points[0];
  const last = points[points.length - 1];
  return `${line} L ${last.x.toFixed(1)} ${height} L ${first.x.toFixed(1)} ${height} Z`;
}

/** Axis labels: the window's start and end, in the user's locale. */
export function timeLabels(samples: Sample[]): { start: string; end: string } {
  if (samples.length === 0) return { start: "", end: "" };
  const format = (value: string) => {
    const at = new Date(value);
    return Number.isNaN(at.getTime()) ? "" : at.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  };
  return { start: format(samples[0].ts), end: format(samples[samples.length - 1].ts) };
}

/** The latest sample, which the tab shows as the current reading. */
export function latest(samples: Sample[]): Sample | null {
  return samples.length > 0 ? samples[samples.length - 1] : null;
}
