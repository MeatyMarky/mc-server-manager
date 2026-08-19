import { AlertTriangle, ChevronDown } from "lucide-react";
import { useId, useState } from "react";

import { errorParts } from "@/lib/ipc";
import { cn } from "@/lib/utils";

/**
 * How a failure is shown anywhere in the app: one plain sentence, the fix
 * underneath it, and the Rust text folded away.
 *
 * The technical half is never the headline — it is behind "Details", which is a
 * real `<button>` with `aria-expanded`, so a screen reader announces the state
 * rather than the chevron.
 */
export function ErrorNotice({
  error,
  className,
  action,
}: {
  error: unknown;
  className?: string;
  /** Optional button, e.g. "Rescan Java" next to a Java error. */
  action?: React.ReactNode;
}) {
  const { message, hint, technical, kind } = errorParts(error);
  const [open, setOpen] = useState(false);
  const detailsId = useId();

  return (
    <div
      role="alert"
      className={cn(
        "rounded-lg border border-destructive/40 bg-destructive/10 p-3 text-sm",
        className,
      )}
    >
      <div className="flex gap-2">
        <AlertTriangle className="mt-0.5 size-4 shrink-0 text-destructive" aria-hidden />
        <div className="min-w-0 flex-1">
          <p className="font-medium">{message}</p>
          {hint ? <p className="mt-1 text-muted-foreground">{hint}</p> : null}

          <div className="mt-2 flex flex-wrap items-center gap-2">
            {action}
            {technical && technical !== message ? (
              <button
                type="button"
                className="inline-flex items-center gap-1 rounded-md px-1.5 py-0.5 text-xs text-muted-foreground underline-offset-2 hover:underline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring"
                aria-expanded={open}
                aria-controls={detailsId}
                onClick={() => setOpen((current) => !current)}
              >
                <ChevronDown
                  className={cn("size-3 transition-transform", open && "rotate-180")}
                  aria-hidden
                />
                Details
              </button>
            ) : null}
          </div>

          {open ? (
            <pre
              id={detailsId}
              className="mt-2 max-h-48 overflow-auto rounded-md bg-background/60 p-2 text-xs whitespace-pre-wrap"
            >
              {technical}
              {"\n"}({kind})
            </pre>
          ) : null}
        </div>
      </div>
    </div>
  );
}
