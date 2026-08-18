import { describe, expect, it } from "vitest";

import type { ParsedLine } from "@/lib/types";
import { MAX_LINES, appendLines, filterLines, historyStep } from "./useConsole";

function line(seq: number, message: string): ParsedLine {
  return {
    seq,
    capturedAt: "2026-08-18T12:00:00Z",
    timestamp: "12:00:00",
    level: "info",
    thread: "Server thread",
    message,
    raw: `[12:00:00] [Server thread/INFO]: ${message}`,
    stderr: false,
  };
}

describe("appendLines", () => {
  it("appends a batch in order", () => {
    const result = appendLines([line(0, "a")], [line(1, "b"), line(2, "c")]);
    expect(result.map((entry) => entry.message)).toEqual(["a", "b", "c"]);
  });

  it("returns the same array for an empty batch", () => {
    const current = [line(0, "a")];
    expect(appendLines(current, [])).toBe(current);
  });

  it("drops the oldest lines past the cap during a chunk-generation flood", () => {
    const existing = Array.from({ length: MAX_LINES }, (_, index) => line(index, `old ${index}`));
    const incoming = Array.from({ length: 500 }, (_, index) =>
      line(MAX_LINES + index, `new ${index}`),
    );

    const result = appendLines(existing, incoming);
    expect(result).toHaveLength(MAX_LINES);
    expect(result[0].message).toBe("old 500");
    expect(result[result.length - 1].message).toBe("new 499");
  });
});

describe("filterLines", () => {
  const lines = [
    line(0, "Preparing spawn area: 42%"),
    line(1, "Notch joined the game"),
    line(2, "Done (7.214s)! For help, type help"),
  ];

  it("returns everything for an empty search", () => {
    expect(filterLines(lines, "   ")).toBe(lines);
  });

  it("matches case-insensitively on the message", () => {
    expect(filterLines(lines, "NOTCH").map((l) => l.seq)).toEqual([1]);
  });

  it("also matches the raw line, so prefixes are searchable", () => {
    expect(filterLines(lines, "server thread")).toHaveLength(3);
  });
});

describe("historyStep", () => {
  const history = ["say one", "say two", "say three"];

  it("walks backwards from the newest command", () => {
    const first = historyStep(history, -1, "up");
    expect(first).toEqual({ index: 2, value: "say three" });
    expect(historyStep(history, first.index, "up")).toEqual({ index: 1, value: "say two" });
    expect(historyStep(history, 1, "up")).toEqual({ index: 0, value: "say one" });
    // Already at the oldest: stays put rather than wrapping.
    expect(historyStep(history, 0, "up")).toEqual({ index: 0, value: "say one" });
  });

  it("walks forward and clears the input at the end", () => {
    expect(historyStep(history, 0, "down")).toEqual({ index: 1, value: "say two" });
    expect(historyStep(history, 2, "down")).toEqual({ index: -1, value: "" });
  });

  it("does nothing when the user is already typing a fresh command", () => {
    expect(historyStep(history, -1, "down")).toEqual({ index: -1, value: null });
  });

  it("handles an empty history", () => {
    expect(historyStep([], -1, "up")).toEqual({ index: -1, value: null });
  });
});
