import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { Readiness } from "@/lib/types";

const startupReadiness = vi.fn();
const javaRescan = vi.fn();
const openUrl = vi.fn();

vi.mock("@/lib/ipc", () => ({
  ipc: {
    startupReadiness: () => startupReadiness(),
    javaRescan: () => javaRescan(),
  },
}));

vi.mock("@tauri-apps/plugin-opener", () => ({
  openUrl: (url: string) => openUrl(url),
}));

// Imported after the mocks so the component picks them up.
const { FirstRun } = await import("./FirstRun");

function readiness(overrides: Partial<Readiness> = {}): Readiness {
  return {
    javaScanPending: false,
    javaCount: 1,
    newestJava: 21,
    recommendedJava: 21,
    warning: null,
    instanceCount: 0,
    ...overrides,
  };
}

function renderFirstRun(onCreate = vi.fn(), onImport = vi.fn()) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <FirstRun onCreate={onCreate} onImport={onImport} />
    </QueryClientProvider>,
  );
  return { onCreate, onImport };
}

describe("FirstRun", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("offers both ways in", async () => {
    startupReadiness.mockResolvedValue(readiness());
    const { onCreate, onImport } = renderFirstRun();
    const user = userEvent.setup();

    await user.click(await screen.findByRole("button", { name: /create a server/i }));
    expect(onCreate).toHaveBeenCalled();

    await user.click(screen.getByRole("button", { name: /import an existing one/i }));
    expect(onImport).toHaveBeenCalled();
  });

  it("says it is still looking rather than claiming there is no Java", async () => {
    // Detection runs in the background at launch, so "not found yet" and "not
    // installed" must not read the same.
    startupReadiness.mockResolvedValue(readiness({ javaScanPending: true, javaCount: 0, newestJava: null }));
    renderFirstRun();

    expect(await screen.findByText(/looking for java/i)).toBeInTheDocument();
    expect(screen.queryByText(/no java was found/i)).toBeNull();
  });

  it("warns when nothing suitable is installed, and offers a way to fix it", async () => {
    startupReadiness.mockResolvedValue(
      readiness({
        javaCount: 0,
        newestJava: null,
        warning: "No Java was found on this computer. A server needs one — install a JDK.",
      }),
    );
    renderFirstRun();
    const user = userEvent.setup();

    expect(await screen.findByText(/no java was found/i)).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: /get a jdk/i }));
    expect(openUrl).toHaveBeenCalledWith(expect.stringContaining("adoptium"));

    javaRescan.mockResolvedValue([]);
    await user.click(screen.getByRole("button", { name: /rescan/i }));
    expect(javaRescan).toHaveBeenCalled();
  });

  it("confirms a usable Java instead of staying silent", async () => {
    startupReadiness.mockResolvedValue(readiness({ newestJava: 25 }));
    renderFirstRun();

    expect(await screen.findByText(/java 25 found/i)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /get a jdk/i })).toBeNull();
  });

  it("says up front that the EULA is the user's to accept", async () => {
    startupReadiness.mockResolvedValue(readiness());
    renderFirstRun();

    expect(await screen.findByText(/never accepts it for you/i)).toBeInTheDocument();
  });
});
