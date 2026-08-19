// How a runtime is described in the picker. Pure, so the wording is testable.
import type { JavaRuntime } from "@/lib/types";

/** Whether this runtime may be used for a server at all. */
export function isUsable(runtime: JavaRuntime): boolean {
  return runtime.valid && runtime.bits === 64;
}

/**
 * Why a runtime is greyed out, or null when it is fine.
 *
 * 32-bit JVMs top out around 1.5 GB of heap and refuse to start with the `-Xmx`
 * a server is normally given, so they are excluded rather than offered and left
 * to fail at launch.
 */
export function unsuitableReason(runtime: JavaRuntime): string | null {
  if (!runtime.valid) return "did not answer -version";
  if (runtime.bits === 64) return null;
  if (runtime.bits === null) return "width unknown until the next scan";
  return "32-bit, not suitable for servers";
}

/** One line for a runtime, as the picker shows it. */
export function runtimeLabel(runtime: JavaRuntime): string {
  const parts = [`Java ${runtime.major}`];
  if (runtime.vendor) parts.push(runtime.vendor);
  parts.push(runtime.path);
  const reason = unsuitableReason(runtime);
  return reason ? `${parts.join(" · ")} — ${reason}` : parts.join(" · ");
}

/**
 * How old the detected list is, in words.
 *
 * A JDK installed after the last scan is simply absent from the picker, and
 * that is impossible to work out from a list that looks complete — so the list
 * says when it was built.
 */
export function scanAgeLabel(lastScanAt: string | null): string {
  if (!lastScanAt) return "Java has not been scanned yet";

  const at = new Date(lastScanAt);
  if (Number.isNaN(at.getTime())) return "Last scan time unknown";

  const minutes = Math.floor((Date.now() - at.getTime()) / 60_000);
  if (minutes < 1) return "Scanned just now";
  if (minutes < 60) return `Scanned ${minutes} minute${minutes === 1 ? "" : "s"} ago`;

  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `Scanned ${hours} hour${hours === 1 ? "" : "s"} ago`;

  const days = Math.floor(hours / 24);
  return `Scanned ${days} day${days === 1 ? "" : "s"} ago`;
}
