// Wording for the Backups tab. Pure functions, so the phrasing is testable.
import type { Backup, Format, Schedule, Scope, SpaceCheck } from "@/lib/types";

export function formatLabel(format: Format): string {
  return format === "zip" ? "zip" : "tar.zst";
}

export function scopeLabel(scope: Scope): string {
  return scope === "worlds" ? "Worlds only" : "Full instance";
}

/** manual / scheduled / pre_restore, as a person would say it. */
export function kindLabel(kind: string): string {
  switch (kind) {
    case "scheduled":
      return "Scheduled";
    case "pre_restore":
      return "Before restore";
    default:
      return "Manual";
  }
}

/** Backups taken automatically before a restore are the safety net, so they are
 *  marked out from the ones the user asked for. */
export function isSafetyCopy(backup: Backup): boolean {
  return backup.kind === "pre_restore";
}

export function whenLabel(iso: string): string {
  const at = new Date(iso);
  if (Number.isNaN(at.getTime())) return iso;
  return at.toLocaleString([], {
    year: "numeric",
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

/** Newest first, which is the order people look for a backup in. */
export function sortForDisplay(backups: Backup[]): Backup[] {
  return [...backups].sort((left, right) => right.createdAt.localeCompare(left.createdAt));
}

/** How a schedule reads in one line. */
export function scheduleSummary(schedule: Schedule): string {
  const cadence = schedule.intervalMinutes
    ? intervalLabel(schedule.intervalMinutes)
    : schedule.cron
      ? `daily at ${schedule.cron}`
      : "no cadence set";

  const retention: string[] = [];
  if (schedule.keepCount) retention.push(`keep ${schedule.keepCount}`);
  if (schedule.keepDays) retention.push(`keep ${schedule.keepDays} days`);

  const extras: string[] = [];
  if (schedule.skipIfIdle) extras.push("skip when idle");
  if (schedule.restartAfter) extras.push("restart after");

  return [
    `${scopeLabel(schedule.scope)} as ${formatLabel(schedule.format)}, ${cadence}`,
    retention.join(" and "),
    extras.join(", "),
  ]
    .filter(Boolean)
    .join(" · ");
}

export function intervalLabel(minutes: number): string {
  if (minutes % (60 * 24) === 0) {
    const days = minutes / (60 * 24);
    return days === 1 ? "every day" : `every ${days} days`;
  }
  if (minutes % 60 === 0) {
    const hours = minutes / 60;
    return hours === 1 ? "every hour" : `every ${hours} hours`;
  }
  return `every ${minutes} minutes`;
}

/** The disk-space verdict, or null when there is nothing to warn about. */
export function spaceWarning(check: SpaceCheck | undefined): string | null {
  if (!check || check.sufficient) return null;
  return check.message ?? "There is not enough free space for this backup.";
}
