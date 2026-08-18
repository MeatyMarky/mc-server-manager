import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import type { InstanceView } from "@/lib/types";
import { InstanceSidebar } from "./InstanceSidebar";

function instance(overrides: Partial<InstanceView> = {}): InstanceView {
  return {
    id: 1,
    uuid: "u1",
    name: "Survival",
    path: "Z:/servers/survival",
    folderExists: true,
    status: "stopped",
    serverType: "paper",
    mcVersion: "1.21.4",
    loaderVersion: null,
    launchKind: "jar",
    launchTarget: "server.jar",
    javaPath: null,
    javaMajor: null,
    jvmArgs: [],
    serverArgs: [],
    minRamMb: 1024,
    maxRamMb: 4096,
    eulaAccepted: false,
    eulaAcceptedAt: null,
    autoStart: false,
    autoRestart: false,
    restartMax: 3,
    restartWindowS: 600,
    stopTimeoutS: 60,
    contentDir: "plugins",
    color: null,
    notes: null,
    lastExitCode: null,
    lastStartedAt: null,
    lastStoppedAt: null,
    pid: null,
    installedAt: null,
    createdAt: "2026-01-01T00:00:00Z",
    updatedAt: "2026-01-01T00:00:00Z",
    ...overrides,
  };
}

const noop = () => {};

describe("InstanceSidebar", () => {
  it("invites the first instance when the list is empty", () => {
    render(
      <InstanceSidebar
        instances={[]}
        isLoading={false}
        selectedId={null}
        onSelect={noop}
        onCreate={noop}
        onImport={noop}
      />,
    );
    expect(screen.getByText(/No instances yet/i)).toBeInTheDocument();
  });

  it("filters by name, version and server type", async () => {
    const user = userEvent.setup();
    render(
      <InstanceSidebar
        instances={[
          instance(),
          instance({ id: 2, uuid: "u2", name: "Creative", serverType: "fabric", mcVersion: "1.20.1" }),
        ]}
        isLoading={false}
        selectedId={1}
        onSelect={noop}
        onCreate={noop}
        onImport={noop}
      />,
    );

    await user.type(screen.getByLabelText("Filter instances"), "fabric");
    expect(screen.getByText("Creative")).toBeInTheDocument();
    expect(screen.queryByText("Survival")).not.toBeInTheDocument();
  });

  it("keeps a missing instance in the list instead of hiding it", () => {
    render(
      <InstanceSidebar
        instances={[instance({ status: "missing", folderExists: false })]}
        isLoading={false}
        selectedId={1}
        onSelect={noop}
        onCreate={noop}
        onImport={noop}
      />,
    );
    expect(screen.getByText("Survival")).toBeInTheDocument();
    expect(screen.getByText("Folder missing")).toBeInTheDocument();
  });

  it("reports the clicked instance", async () => {
    const user = userEvent.setup();
    const onSelect = vi.fn();
    render(
      <InstanceSidebar
        instances={[instance(), instance({ id: 2, uuid: "u2", name: "Creative" })]}
        isLoading={false}
        selectedId={1}
        onSelect={onSelect}
        onCreate={noop}
        onImport={noop}
      />,
    );

    await user.click(screen.getByText("Creative"));
    expect(onSelect).toHaveBeenCalledWith(2);
  });
});
