import { Construction } from "lucide-react";

/** Tabs that exist in the shell but are filled in by a later phase. */
export function PlaceholderTab({
  title,
  phase,
  description,
}: {
  title: string;
  phase: number;
  description: string;
}) {
  return (
    <div className="flex h-full items-center justify-center rounded-lg border border-dashed border-border p-10">
      <div className="max-w-md text-center">
        <Construction className="mx-auto mb-3 size-6 text-muted-foreground" />
        <h3 className="text-sm font-semibold">{title}</h3>
        <p className="mt-2 text-sm text-muted-foreground">{description}</p>
        <p className="mt-3 text-xs text-muted-foreground">Arrives in phase {phase}.</p>
      </div>
    </div>
  );
}
