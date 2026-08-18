import { describe, expect, it } from "vitest";

import type { InstanceStatus } from "./types";
import {
  SERVER_TYPES,
  SERVER_TYPE_LABEL,
  STATUS_COLOR,
  STATUS_LABEL,
  needsLocating,
  statusIsLive,
  statusPulses,
} from "./status";

const ALL_STATUSES: InstanceStatus[] = [
  "stopped",
  "starting",
  "running",
  "stopping",
  "crashed",
  "unmanaged",
  "missing",
];

describe("status presentation", () => {
  it("labels and colours every status the backend can send", () => {
    for (const status of ALL_STATUSES) {
      expect(STATUS_LABEL[status]).toBeTruthy();
      expect(STATUS_COLOR[status]).toMatch(/^var\(--status-/);
    }
  });

  it("treats adopted orphans as live so they cannot be deleted mid-run", () => {
    expect(statusIsLive("unmanaged")).toBe(true);
    expect(statusIsLive("running")).toBe(true);
    expect(statusIsLive("starting")).toBe(true);
    expect(statusIsLive("stopping")).toBe(true);
    expect(statusIsLive("crashed")).toBe(false);
    expect(statusIsLive("stopped")).toBe(false);
    expect(statusIsLive("missing")).toBe(false);
  });

  it("only animates transitional states", () => {
    expect(statusPulses("starting")).toBe(true);
    expect(statusPulses("stopping")).toBe(true);
    expect(statusPulses("running")).toBe(false);
  });

  it("routes a missing folder to recovery rather than to an error", () => {
    expect(needsLocating("missing")).toBe(true);
    expect(needsLocating("crashed")).toBe(false);
  });

  it("labels every server type the backend supports", () => {
    expect(SERVER_TYPES).toHaveLength(6);
    for (const type of SERVER_TYPES) {
      expect(SERVER_TYPE_LABEL[type]).toBeTruthy();
    }
    // The Rust enum serializes NeoForge as neo_forge; the label stays branded.
    expect(SERVER_TYPE_LABEL.neo_forge).toBe("NeoForge");
  });
});
