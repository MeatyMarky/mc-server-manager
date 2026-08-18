import { ArrowDownToLine, Copy, Search, Send } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Badge, Switch } from "@/components/ui/misc";
import { Label } from "@/components/ui/dialog";
import { InstallPanel } from "@/features/setup/InstallPanel";
import { errorMessage } from "@/lib/ipc";
import type { InstanceView, LogLevel } from "@/lib/types";
import { cn } from "@/lib/utils";
import { filterLines, historyStep, useConsole } from "./useConsole";

const LEVEL_CLASS: Record<LogLevel, string> = {
  trace: "text-muted-foreground",
  debug: "text-muted-foreground",
  info: "text-foreground",
  warn: "text-[var(--status-starting)]",
  error: "text-destructive",
  fatal: "text-destructive font-medium",
  raw: "text-muted-foreground",
};

export function ConsoleTab({ instance }: { instance: InstanceView }) {
  const { lines, history, send } = useConsole(instance.id, instance.uuid);
  const [search, setSearch] = useState("");
  const [autoscroll, setAutoscroll] = useState(true);
  const [draft, setDraft] = useState("");
  const [historyIndex, setHistoryIndex] = useState(-1);
  const scroller = useRef<HTMLDivElement>(null);

  const visible = useMemo(() => filterLines(lines, search), [lines, search]);

  useEffect(() => {
    if (!autoscroll || !scroller.current) return;
    scroller.current.scrollTop = scroller.current.scrollHeight;
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
      toast.error(errorMessage(error));
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
          onClick={() => {
            void navigator.clipboard.writeText(visible.map((line) => line.raw).join("\n"));
            toast.success(search ? "Copied the filtered lines" : "Copied the console");
          }}
        >
          <Copy /> Copy
        </Button>
      </div>

      <div
        ref={scroller}
        tabIndex={0}
        role="log"
        aria-label="Server console"
        className="min-h-64 flex-1 overflow-y-auto rounded-md border border-border bg-card/40 p-3 font-mono text-xs leading-relaxed"
        onScroll={(event) => {
          // Scrolling away from the bottom turns autoscroll off, the way a
          // terminal does; scrolling back to the bottom turns it on again.
          const element = event.currentTarget;
          const atBottom =
            element.scrollHeight - element.scrollTop - element.clientHeight < 24;
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
