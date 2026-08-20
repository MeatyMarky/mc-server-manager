import { ArrowDownToLine, Copy, Search, Send } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";

import { Button } from "@/components/ui/button";
import { ErrorNotice } from "@/components/ui/ErrorNotice";
import { Input } from "@/components/ui/input";
import { Badge, Switch } from "@/components/ui/misc";
import { Label } from "@/components/ui/dialog";
import { InstallPanel } from "@/features/setup/InstallPanel";
import { copyToClipboard } from "@/lib/clipboard";
import { toastError } from "@/lib/toast";
import type { InstanceView, LogLevel } from "@/lib/types";
import { cn } from "@/lib/utils";
import { filterLines, historyStep, isAtBottom, useConsole } from "./useConsole";

const LEVEL_CLASS: Record<LogLevel, string> = {
  trace: "text-muted-foreground",
  debug: "text-muted-foreground",
  info: "text-foreground",
  warn: "text-[var(--status-starting)]",
  error: "text-destructive",
  fatal: "text-destructive font-medium",
  raw: "text-muted-foreground",
};

export function ConsoleTab({
  instance,
  startError,
}: {
  instance: InstanceView;
  /// The last thing that stopped this server from starting, if any.
  startError?: unknown;
}) {
  const { lines, history, send } = useConsole(instance.id, instance.uuid);
  const [search, setSearch] = useState("");
  const [autoscroll, setAutoscroll] = useState(true);
  const [draft, setDraft] = useState("");
  const [historyIndex, setHistoryIndex] = useState(-1);
  const scroller = useRef<HTMLDivElement>(null);
  // Set while this component is the one moving the scrollbar. Without it the
  // jump to the bottom fires `onScroll`, that handler sees "at the bottom" and
  // turns autoscroll back on — so on a server printing continuously, scrolling
  // up releases autoscroll for a few milliseconds and is then undone by the
  // next batch. Scrolling away becomes impossible, which is exactly the
  // symptom: every new line snaps the view back down.
  const selfScrolling = useRef(false);

  const visible = useMemo(() => filterLines(lines, search), [lines, search]);

  useEffect(() => {
    const element = scroller.current;
    if (!autoscroll || !element) return;

    // Only arm the guard when the view really moves. A jump that changes
    // nothing produces no scroll event, and a flag left standing would swallow
    // the user's next scroll instead of the app's own.
    if (Math.abs(element.scrollTop - element.scrollHeight) <= 1) return;

    selfScrolling.current = true;
    element.scrollTop = element.scrollHeight;

    // The browser delivers the matching event on a later frame. Clearing the
    // flag on the next frame as well means it can never outlive the jump, even
    // if that event never comes.
    const frame = requestAnimationFrame(() => {
      selfScrolling.current = false;
    });
    return () => cancelAnimationFrame(frame);
  }, [visible, autoscroll]);

  const canSend = instance.status === "running" || instance.status === "starting";

  async function submit(event: React.FormEvent) {
    event.preventDefault();
    const command = draft;
    setDraft("");
    setHistoryIndex(-1);
    try {
      await send(command);
    } catch (error) {
      toastError(error);
    }
  }

  function onKeyDown(event: React.KeyboardEvent<HTMLInputElement>) {
    if (event.key !== "ArrowUp" && event.key !== "ArrowDown") return;
    event.preventDefault();
    const step = historyStep(history, historyIndex, event.key === "ArrowUp" ? "up" : "down");
    setHistoryIndex(step.index);
    if (step.value !== null) setDraft(step.value);
  }

  return (
    <div className="flex h-full min-h-0 flex-col gap-3">
      {instance.installedAt ? null : <InstallPanel instance={instance} />}

      <div className="flex flex-wrap items-center gap-2">
        <div className="relative min-w-48 flex-1">
          <Search className="pointer-events-none absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground" />
          <Input
            className="h-8 pl-8 text-xs"
            placeholder="Search console"
            aria-label="Search console"
            value={search}
            onChange={(event) => setSearch(event.target.value)}
          />
        </div>

        {search ? (
          <Badge>
            {visible.length} of {lines.length}
          </Badge>
        ) : (
          <Badge>{lines.length} lines</Badge>
        )}

        <div className="flex items-center gap-2">
          <Label htmlFor="console-autoscroll" className="text-xs text-muted-foreground">
            <ArrowDownToLine className="mr-1 inline size-3.5" />
            Autoscroll
          </Label>
          <Switch
            id="console-autoscroll"
            checked={autoscroll}
            onCheckedChange={setAutoscroll}
          />
        </div>

        <Button
          type="button"
          variant="outline"
          size="sm"
          onClick={() =>
            void copyToClipboard(
              visible.map((line) => line.raw).join("\n"),
              search ? `${visible.length} matching lines` : `${visible.length} console lines`,
            )
          }
        >
          <Copy /> Copy
        </Button>
      </div>

      {startError ? <ErrorNotice error={startError} className="mb-3" /> : null}

      <div
        ref={scroller}
        tabIndex={0}
        role="log"
        aria-label="Server console"
        // A busy server prints thousands of lines a second. `additions text`
        // keeps a screen reader announcing new output instead of re-reading the
        // whole buffer, and the region stays focusable so it can be reviewed at
        // the user's own pace.
        aria-relevant="additions text"
        aria-atomic="false"
        // `min-h-0` so the console can shrink inside the tab at a small window
        // height instead of pushing its own scrollbar out of reach, and
        // `overscroll-contain` so reaching the top does not scroll the page
        // behind it.
        className="min-h-0 flex-1 overflow-y-auto overscroll-contain rounded-md border border-border bg-card/40 p-3 font-mono text-xs leading-relaxed"
        onScroll={(event) => {
          // The jump this component just made is not the user scrolling.
          if (selfScrolling.current) {
            selfScrolling.current = false;
            return;
          }
          // Anything else is: scrolling away from the bottom releases
          // autoscroll the way a terminal does, and coming back re-arms it.
          const atBottom = isAtBottom(event.currentTarget);
          if (atBottom !== autoscroll) setAutoscroll(atBottom);
        }}
      >
        {visible.length === 0 ? (
          <p className="text-muted-foreground">
            {lines.length === 0
              ? "No output yet. Start the server to see its console here."
              : "No lines match that search."}
          </p>
        ) : (
          visible.map((line) => (
            <div key={line.seq} className={cn("whitespace-pre-wrap", LEVEL_CLASS[line.level])}>
              {/* Colour alone carries the level for sighted users; this says it
                  out loud for everyone else. */}
              {line.level === "error" || line.level === "warn" ? (
                <span className="sr-only">{line.level === "error" ? "Error: " : "Warning: "}</span>
              ) : null}
              {line.timestamp ? (
                <span className="text-muted-foreground">[{line.timestamp}] </span>
              ) : null}
              {line.thread ? (
                <span className="text-muted-foreground">[{line.thread}] </span>
              ) : null}
              {line.message}
            </div>
          ))
        )}
      </div>

      <form className="flex gap-2" onSubmit={submit}>
        <Input
          className="font-mono text-xs"
          placeholder={canSend ? "Type a command, e.g. say hello" : "Start the server to send commands"}
          aria-label="Server command"
          disabled={!canSend}
          value={draft}
          onChange={(event) => setDraft(event.target.value)}
          onKeyDown={onKeyDown}
        />
        <Button type="submit" size="sm" disabled={!canSend || !draft.trim()}>
          <Send /> Send
        </Button>
      </form>
    </div>
  );
}
