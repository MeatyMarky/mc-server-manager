import { describe, expect, it } from "vitest";

import type { JavaRuntime } from "@/lib/types";

import { isUsable, runtimeLabel, unsuitableReason } from "./javaLabels";

function runtime(overrides: Partial<JavaRuntime> = {}): JavaRuntime {
  return {
    id: 1,
    path: "C:/Program Files/Java/jdk-21/bin/java.exe",
    major: 21,
    fullVersion: "21.0.10",
    vendor: "Temurin",
    arch: "x64",
    bits: 64,
    source: "common_dir",
    valid: true,
    detectedAt: "2026-08-19T10:00:00Z",
    ...overrides,
  };
}

describe("java runtime labels", () => {
  it("accepts a 64-bit runtime without comment", () => {
    const jdk = runtime();
    expect(isUsable(jdk)).toBe(true);
    expect(unsuitableReason(jdk)).toBeNull();
    expect(runtimeLabel(jdk)).toBe(
      "Java 21 · Temurin · C:/Program Files/Java/jdk-21/bin/java.exe",
    );
  });

  it("greys a 32-bit runtime and says why", () => {
    // The Program Files (x86) Java 8 that caused "Invalid maximum heap size".
    const x86 = runtime({
      path: "C:/Program Files (x86)/Java/jre1.8.0_451/bin/java.exe",
      major: 8,
      bits: 32,
      arch: "x86",
    });

    expect(isUsable(x86)).toBe(false);
    expect(unsuitableReason(x86)).toBe("32-bit, not suitable for servers");
    expect(runtimeLabel(x86)).toContain("32-bit, not suitable for servers");
  });

  it("treats an unknown width as unusable rather than assuming 64-bit", () => {
    // Rows detected before bitness was recorded; assuming was the bug.
    const old = runtime({ bits: null });
    expect(isUsable(old)).toBe(false);
    expect(unsuitableReason(old)).toBe("width unknown until the next scan");
  });

  it("reports a runtime that did not answer at all", () => {
    const broken = runtime({ valid: false, bits: null });
    expect(isUsable(broken)).toBe(false);
    expect(unsuitableReason(broken)).toBe("did not answer -version");
  });
});
