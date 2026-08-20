import { Loader2 } from "lucide-react";
import { useMemo, useState } from "react";

import { Button } from "@/components/ui/button";
import { ErrorNotice } from "@/components/ui/ErrorNotice";
import { useProviderVersions } from "@/features/setup/queries";
import type { ServerType, VersionEntry, VersionKind } from "@/lib/types";

/** The three filters, in the order they are offered. */
const FILTERS: { kind: VersionKind; label: string; hint: string }[] = [
  { kind: "release", label: "Releases", hint: "Finished versions." },
  { kind: "snapshot", label: "Snapshots", hint: "Weekly development builds." },
  {
    kind: "pre_release",
    label: "Pre-releases",
    hint: "The run-up to a release, including release candidates.",
  },
];

const KIND_LABEL: Record<VersionKind, string> = {
  release: "Release",
  snapshot: "Snapshot",
  pre_release: "Pre-release",
  ancient: "Alpha/Beta",
};

/**
 * The version picker, as a table.
 *
 * A dropdown of two hundred versions is a scroll bar and a guess. A table with
 * dates answers the question people actually arrive with — "the one from last
 * March", "whatever we were on before the update" — and the filters are the
 * manifest's own kinds rather than a guess made from the version string.
 *
 * Rows are radio buttons, so arrow keys move the selection and the browser
 * handles the focus ring for free.
 */
export function VersionTable({
  serverType,
  value,
  onChange,
  disabled,
}: {
  serverType: ServerType;
  value: string;
  onChange: (version: string) => void;
  disabled?: boolean;
}) {
  const versions = useProviderVersions(serverType);
  const [kinds, setKinds] = useState<Set<VersionKind>>(() => new Set<VersionKind>(["release"]));

  const rows = useMemo(() => filterVersions(versions.data ?? [], kinds), [versions.data, kinds]);

  function toggle(kind: VersionKind) {
    setKinds((current) => {
      const next = new Set(current);
      if (next.has(kind)) next.delete(kind);
      else next.add(kind);
      // Never leave the table with nothing to show and no way back.
      if (next.size === 0) next.add("release");
      return next;
    });
  }

  return (
    <div className="grid gap-2">
      <div className="flex flex-wrap items-center gap-4">
        {FILTERS.map((filter) => (
          <label key={filter.kind} className="flex items-center gap-2 text-sm" title={filter.hint}>
            <input
              type="checkbox"
              className="size-4 accent-primary focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring"
              checked={kinds.has(filter.kind)}
              disabled={disabled}
              onChange={() => toggle(filter.kind)}
            />
            {filter.label}
          </label>
        ))}
        <span className="ml-auto text-xs text-muted-foreground">
          {versions.isLoading ? "" : `${rows.length} version${rows.length === 1 ? "" : "s"}`}
        </span>
      </div>

      {versions.error ? (
        <ErrorNotice
          error={versions.error}
          action={
            <Button variant="outline" size="sm" onClick={() => void versions.refetch()}>
              Try again
            </Button>
          }
        />
      ) : (
        <div className="max-h-64 overflow-y-auto overscroll-contain rounded-md border border-border">
          {versions.isLoading ? (
            <p className="flex items-center gap-2 p-4 text-sm text-muted-foreground">
              <Loader2 className="size-4 animate-spin" aria-hidden />
              Asking {serverType} which versions it has…
            </p>
          ) : rows.length === 0 ? (
            <p className="p-4 text-sm text-muted-foreground">
              Nothing matches those filters.
            </p>
          ) : (
            <table className="w-full border-collapse text-sm">
              <caption className="sr-only">
                Minecraft versions, newest first. Use the arrow keys to choose one.
              </caption>
              <thead className="sticky top-0 bg-card">
                <tr className="border-b border-border text-left text-xs text-muted-foreground">
                  <th scope="col" className="w-8" />
                  <th scope="col" className="px-2 py-1.5 font-medium">
                    Version
                  </th>
                  <th scope="col" className="px-2 py-1.5 font-medium">
                    Released
                  </th>
                  <th scope="col" className="px-2 py-1.5 font-medium">
                    Type
                  </th>
                </tr>
              </thead>
              <tbody>
                {rows.map((entry) => (
                  <Row
                    key={entry.id}
                    entry={entry}
                    selected={entry.id === value}
                    disabled={disabled}
                    onSelect={() => onChange(entry.id)}
                  />
                ))}
              </tbody>
            </table>
          )}
        </div>
      )}
    </div>
  );
}

function Row({
  entry,
  selected,
  disabled,
  onSelect,
}: {
  entry: VersionEntry;
  selected: boolean;
  disabled?: boolean;
  onSelect: () => void;
}) {
  return (
    <tr
      className={
        selected
          ? "cursor-pointer border-b border-border bg-primary/15"
          : "cursor-pointer border-b border-border hover:bg-muted/40"
      }
      onClick={() => !disabled && onSelect()}
    >
      <td className="px-2 py-1.5">
        <input
          type="radio"
          name="mc-version"
          className="size-4 accent-primary focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring"
          checked={selected}
          disabled={disabled}
          onChange={onSelect}
          aria-label={`Minecraft ${entry.id}`}
        />
      </td>
      <td className="px-2 py-1.5 font-medium">{entry.id}</td>
      <td className="px-2 py-1.5 text-muted-foreground">{releaseDate(entry.releaseTime)}</td>
      <td className="px-2 py-1.5 text-muted-foreground">{KIND_LABEL[entry.kind]}</td>
    </tr>
  );
}

/**
 * The rows the chosen filters leave, in the order the backend sent them.
 *
 * Order is release chronology, decided in Rust from Mojang's manifest — the
 * table never re-sorts, because the two numbering eras cannot be compared as
 * strings and a client-side sort would get 26.2 and 1.21.11 the wrong way
 * round.
 */
export function filterVersions(
  entries: VersionEntry[],
  kinds: ReadonlySet<VersionKind>,
): VersionEntry[] {
  return entries.filter((entry) => kinds.has(entry.kind));
}

/**
 * The date a version came out.
 *
 * Versions a provider publishes but Mojang's manifest does not list have none,
 * and an em dash is more honest than inventing one.
 */
export function releaseDate(releaseTime: string | null): string {
  if (!releaseTime) return "—";
  const at = new Date(releaseTime);
  if (Number.isNaN(at.getTime())) return "—";
  return at.toLocaleDateString(undefined, { year: "numeric", month: "short", day: "numeric" });
}
