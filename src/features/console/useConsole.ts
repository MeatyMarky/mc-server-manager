// Console state: the initial tail from the backend, then batched
// `instance://console` events appended in place. The buffer is capped so a
// server generating chunks cannot grow the page without bound.
import { useCallback, useEffect, useRef, useState } from "react";

import { onConsoleLines } from "@/lib/events";
import { ipc } from "@/lib/ipc";
import type { ParsedLine } from "@/lib/types";

/** Matches the backend ring buffer, so scrollback is the same on both sides. */
export const MAX_LINES = 5_000;

/** Appends a batch, dropping the oldest lines past the cap. */
export function appendLines(current: ParsedLine[], incoming: ParsedLine[]): ParsedLine[] {
  if (incoming.length === 0) return current;
  const merged = current.concat(incoming);
  return merged.length > MAX_LINES ? merged.slice(merged.length - MAX_LINES) : merged;
}

/** Case-insensitive substring match over the message and the raw line. */
export function filterLines(lines: ParsedLine[], search: string): ParsedLine[] {
  const needle = search.trim().toLowerCase();
  if (!needle) return lines;
  return lines.filter(
    (line) =>
      line.message.toLowerCase().includes(needle) ||
      line.raw.toLowerCase().includes(needle),
  );
}

export function useConsole(instanceId: number, instanceUuid: string) {
  const [lines, setLines] = useState<ParsedLine[]>([]);
  const [history, setHistory] = useState<string[]>([]);
  // Buffers arriving between renders; flushed on an animation frame so a burst
  // of batches still costs one render.
  const pending = useRef<ParsedLine[]>([]);
  const frame = useRef<number | null>(null);

  useEffect(() => {
    let active = true;
    setLines([]);
    void ipc.consoleTail(instanceId, 1000).then((tail) => {
      if (active) setLines(tail);
    });
    void ipc.commandHistory(instanceId).then((entries) => {
      if (active) setHistory(entries);
    });
    return () => {
      active = false;
    };
  }, [instanceId]);

  useEffect(() => {
    const flush = () => {
      frame.current = null;
      const batch = pending.current;
      pending.current = [];
      if (batch.length > 0) setLines((current) => appendLines(current, batch));
    };

    const pendingUnlisten = onConsoleLines((payload) => {
      if (payload.uuid !== instanceUuid) return;
      pending.current.push(...payload.lines);
      if (frame.current === null) {
        frame.current = requestAnimationFrame(flush);
      }
    });

    return () => {
      void pendingUnlisten.then((unlisten) => unlisten());
      if (frame.current !== null) cancelAnimationFrame(frame.current);
      frame.current = null;
      pending.current = [];
    };
  }, [instanceUuid]);

  const send = useCallback(
    async (command: string) => {
      const trimmed = command.trim();
      if (!trimmed) return;
      await ipc.instanceSendCommand(instanceId, trimmed);
      setHistory((current) => [...current, trimmed].slice(-100));
    },
    [instanceId],
  );

  return { lines, history, send };
}

/**
 * Up/down recall over the command history. Index -1 means "typing a new
 * command"; the draft is preserved so arrowing back down restores it.
 */
export function historyStep(
  history: string[],
  index: number,
  direction: "up" | "down",
): { index: number; value: string | null } {
  if (history.length === 0) return { index: -1, value: null };

  if (direction === "up") {
    const next = index < 0 ? history.length - 1 : Math.max(0, index - 1);
    return { index: next, value: history[next] };
  }

  if (index < 0) return { index: -1, value: null };
  const next = index + 1;
  if (next >= history.length) return { index: -1, value: "" };
  return { index: next, value: history[next] };
}
