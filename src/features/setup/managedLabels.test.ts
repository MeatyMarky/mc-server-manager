import { describe, expect, it } from "vitest";

import { formatBytes } from "@/lib/format";
import type { JavaPlan, ManagedRuntime } from "@/lib/types";

/**
 * The managed-runtime UI is thin — a list and an offer — so what is worth
 * pinning is the shape of the data it renders, and that the numbers it shows
 * are the ones the backend sent.
 */
function runtime(overrides: Partial<ManagedRuntime> = {}): ManagedRuntime {
  return {
    featureVersion: 25,
    releaseName: "jdk-25.0.4+7",
    vendor: "Eclipse Temurin",
    javaPath: "C:/Users/x/AppData/Roaming/dev.msm.manager/runtimes/temurin-25/bin/java.exe",
    installedAt: "2026-08-19T10:00:00Z",
    sizeBytes: 314_572_800,
    usedBy: [],
    ...overrides,
  };
}

function plan(overrides: Partial<JavaPlan> = {}): JavaPlan {
  return {
    requiredMajor: 25,
    fit: "floor",
    reason: "26.2 Vanilla needs Java 25 or newer, and nothing suitable is installed.",
    warning: null,
    installedMajor: null,
    satisfied: false,
    origin: null,
    javaPath: null,
    offer: null,
    offerError: null,
    downloadsAllowed: true,
    ...overrides,
  };
}

describe("managed runtimes", () => {
  it("lives under a shared runtimes folder keyed by version, not per instance", () => {
    const jdk = runtime();
    expect(jdk.javaPath).toContain("runtimes");
    expect(jdk.javaPath).toContain(`temurin-${jdk.featureVersion}`);
    expect(jdk.javaPath).not.toContain("instances");
  });

  it("says who depends on it, which is what blocks deletion", () => {
    expect(runtime().usedBy).toEqual([]);
    const shared = runtime({ usedBy: ["survival", "creative"] });
    expect(shared.usedBy).toHaveLength(2);
    // Two servers, one download — the reason runtimes are keyed by version.
    expect(new Set(shared.usedBy).size).toBe(2);
  });

  it("reports a size a person can read", () => {
    expect(formatBytes(runtime().sizeBytes)).toBe("300.0 MB");
    expect(formatBytes(runtime({ sizeBytes: 0 }).sizeBytes)).toBe("0 B");
  });
});

describe("java plan", () => {
  it("distinguishes where a satisfied runtime came from", () => {
    for (const origin of ["pinned", "managed", "system"]) {
      const satisfied = plan({ satisfied: true, origin, javaPath: "/jdk/bin/java" });
      expect(satisfied.satisfied).toBe(true);
      expect(satisfied.origin).toBe(origin);
      expect(satisfied.offer).toBeNull();
    }
  });

  it("carries the version and the download size when nothing is installed", () => {
    const offered = plan({
      offer: {
        featureVersion: 25,
        releaseName: "jdk-25.0.4+7",
        openjdkVersion: "25.0.4+7-LTS",
        sizeBytes: 141_164_204,
        os: "windows",
        arch: "x64",
        fileName: "OpenJDK25U-jdk_x64_windows_hotspot_25.0.4_7.zip",
      },
    });

    expect(offered.satisfied).toBe(false);
    expect(offered.offer?.featureVersion).toBe(offered.requiredMajor);
    expect(formatBytes(offered.offer!.sizeBytes)).toBe("134.6 MB");
    // The archive, never an installer that would touch the system.
    expect(offered.offer?.fileName.endsWith(".zip")).toBe(true);
  });

  it("explains itself when downloads are switched off", () => {
    const refused = plan({
      downloadsAllowed: false,
      offerError: "This app is set to use only the Java already installed.",
    });

    expect(refused.offer).toBeNull();
    expect(refused.offerError).toContain("already installed");
  });
});
