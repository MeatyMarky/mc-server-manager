import { describe, expect, it } from "vitest";

import type { PropertyEntry } from "@/lib/types";
import { matchesSearch } from "./ConfigTab";

function entry(key: string, value: string, description: string): PropertyEntry {
  return {
    key,
    value,
    info: {
      key,
      kind: { kind: "text" },
      description,
      default: null,
      known: true,
      group: "Gameplay",
    },
  };
}

describe("matchesSearch", () => {
  const pvp = entry("pvp", "true", "Allow players to damage each other.");
  const motd = entry("motd", "Čajovna", "Message shown in the server list.");

  it("keeps everything when the search is empty", () => {
    expect(matchesSearch(pvp, "")).toBe(true);
    expect(matchesSearch(pvp, "   ")).toBe(true);
  });

  it("matches the key", () => {
    expect(matchesSearch(pvp, "PVP")).toBe(true);
    expect(matchesSearch(motd, "pvp")).toBe(false);
  });

  it("matches the description, so people can search for what a setting does", () => {
    expect(matchesSearch(pvp, "damage")).toBe(true);
    expect(matchesSearch(motd, "server list")).toBe(true);
  });

  it("matches the current value, including non-ASCII", () => {
    expect(matchesSearch(motd, "čajovna")).toBe(true);
    expect(matchesSearch(pvp, "true")).toBe(true);
  });
});
