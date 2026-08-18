import { describe, expect, it } from "vitest";

import { formatBytes, progressPercent } from "./format";

describe("formatBytes", () => {
  it("keeps small sizes in bytes", () => {
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(512)).toBe("512 B");
  });

  it("scales up through the units", () => {
    expect(formatBytes(2048)).toBe("2.0 KB");
    expect(formatBytes(51_437_498)).toBe("49.1 MB");
    expect(formatBytes(5 * 1024 ** 3)).toBe("5.0 GB");
  });

  it("does not produce NaN for nonsense input", () => {
    expect(formatBytes(Number.NaN)).toBe("0 B");
    expect(formatBytes(-5)).toBe("0 B");
  });
});

describe("progressPercent", () => {
  it("returns null while the total is unknown", () => {
    expect(progressPercent(100, null)).toBeNull();
    expect(progressPercent(100, 0)).toBeNull();
  });

  it("clamps to the 0-100 range", () => {
    expect(progressPercent(0, 200)).toBe(0);
    expect(progressPercent(100, 200)).toBe(50);
    expect(progressPercent(300, 200)).toBe(100);
  });
});
