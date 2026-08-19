import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect } from "react";

import { Button } from "@/components/ui/button";
import { onMetrics } from "@/lib/events";
import { formatBytes } from "@/lib/format";
import { ipc } from "@/lib/ipc";
import type { InstanceView, MetricsWindow, Sample } from "@/lib/types";

import { CHART_HEIGHT, CHART_WIDTH, areaPath, latest, linePath, timeLabels, toSeries } from "./chart";

const WINDOWS: { value: MetricsWindow; label: string }[] = [
  { value: "hour", label: "1h" },
  { value: "day", label: "24h" },
  { value: "week", label: "7d" },
  { value: "month", label: "30d" },
];

export function MetricsPanel({
  instance,
  window: selected,
  onWindowChange,
}: {
  instance: InstanceView;
  window: MetricsWindow;
  onWindowChange: (window: MetricsWindow) => void;
}) {
  const queryClient = useQueryClient();

  const samples = useQuery({
    queryKey: ["metrics", instance.id, selected],
    queryFn: () => ipc.metricsRange(instance.id, selected),
  });

  const heap = useQuery({
    queryKey: ["heap", instance.id],
    queryFn: () => ipc.metricsHeapBytes(instance.id),
  });

  // Each sample the collector takes is pushed, so the live view refreshes
  // without a polling interval.
  useEffect(() => {
    const pending = onMetrics((payload) => {
      if (payload.uuid !== instance.uuid) return;
      queryClient.setQueryData<Sample[]>(["metrics", instance.id, selected], (current) => {
        const next = [...(current ?? []), payload];
        // The window's worth of points is all the chart can show anyway.
        return next.length > 2000 ? next.slice(next.length - 2000) : next;
      });
    });
    return () => void pending.then((unlisten) => unlisten());
  }, [instance.id, instance.uuid, queryClient, selected]);

  const rows = samples.data ?? [];
  const now = latest(rows);
  const labels = timeLabels(rows);
  const heapBytes = heap.data ?? null;

  return (
    <section className="space-y-4">
      <header className="flex flex-wrap items-center justify-between gap-2">
        <div>
          <h3 className="text-sm font-medium">Resources</h3>
          <p className="text-xs text-muted-foreground">
            {now
              ? `${now.cpuPct.toFixed(0)}% CPU · ${formatBytes(now.rssBytes)}${
                  heapBytes ? ` of ${formatBytes(heapBytes)} heap` : ""
                }${now.players === null ? "" : ` · ${now.players} online`}`
              : "No samples yet. Samples are taken while a server runs."}
          </p>
        </div>
        <div className="flex gap-1" role="group" aria-label="Time window">
          {WINDOWS.map((option) => (
            <Button
              key={option.value}
              size="sm"
              variant={option.value === selected ? "default" : "outline"}
              aria-pressed={option.value === selected}
              onClick={() => onWindowChange(option.value)}
            >
              {option.label}
            </Button>
          ))}
        </div>
      </header>

      <Chart
        title="CPU"
        caption={now ? `${now.cpuPct.toFixed(0)}%` : "—"}
        labels={labels}
        samples={rows}
        value={(sample) => sample.cpuPct}
        format={(value) => `${Math.round(value)}%`}
        filled
      />

      <Chart
        title={heapBytes ? "Memory vs allocated heap" : "Memory"}
        caption={now ? formatBytes(now.rssBytes) : "—"}
        labels={labels}
        samples={rows}
        value={(sample) => sample.rssBytes}
        format={formatBytes}
        // A fixed ceiling makes "using half its heap" readable at a glance.
        max={heapBytes ?? undefined}
        filled
      />

      <Chart
        title="Players"
        caption={now?.players === null || now === null ? "—" : String(now.players)}
        labels={labels}
        samples={rows}
        value={(sample) => sample.players}
        format={(value) => String(Math.round(value))}
        min={4}
      />
    </section>
  );
}

function Chart({
  title,
  caption,
  labels,
  samples,
  value,
  format,
  max,
  min,
  filled = false,
}: {
  title: string;
  caption: string;
  labels: { start: string; end: string };
  samples: Sample[];
  value: (sample: Sample) => number | null;
  format: (value: number) => string;
  max?: number;
  min?: number;
  filled?: boolean;
}) {
  const series = toSeries(samples, value, { max, min });
  const empty = series.points.length === 0;

  return (
    <figure className="rounded-lg border border-border bg-card p-3">
      <figcaption className="mb-2 flex items-baseline justify-between text-xs">
        <span className="font-medium">{title}</span>
        <span className="text-muted-foreground">{caption}</span>
      </figcaption>

      {empty ? (
        <div className="flex h-[140px] items-center justify-center text-xs text-muted-foreground">
          Nothing recorded in this window
        </div>
      ) : (
        <svg
          viewBox={`0 0 ${CHART_WIDTH} ${CHART_HEIGHT}`}
          className="h-[140px] w-full"
          role="img"
          aria-label={`${title}: ${caption}`}
          preserveAspectRatio="none"
        >
          {[0.25, 0.5, 0.75].map((fraction) => (
            <line
              key={fraction}
              x1={0}
              x2={CHART_WIDTH}
              y1={CHART_HEIGHT * fraction}
              y2={CHART_HEIGHT * fraction}
              className="stroke-border"
              strokeWidth={1}
              vectorEffect="non-scaling-stroke"
            />
          ))}
          {filled ? (
            <path d={areaPath(series.points)} className="fill-primary/15" />
          ) : null}
          <path
            d={linePath(series.points)}
            fill="none"
            className="stroke-primary"
            strokeWidth={1.5}
            vectorEffect="non-scaling-stroke"
          />
        </svg>
      )}

      <div className="mt-1 flex justify-between text-[10px] text-muted-foreground">
        <span>{labels.start}</span>
        <span>{format(series.max)}</span>
        <span>{labels.end}</span>
      </div>
    </figure>
  );
}
