import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";

import { errorParts } from "@/lib/ipc";

import { ErrorNotice } from "./ErrorNotice";

const javaError = {
  kind: "java_too_old",
  message: "Minecraft 1.21.4 needs Java 21, but the newest Java on this computer is Java 17.",
  hint: "Install Java 21 or newer, then use Rescan in Settings.",
  technical: "Minecraft 1.21.4 needs Java 21; the newest found is Java 17",
};

describe("errorParts", () => {
  it("keeps the readable half and the technical half apart", () => {
    const parts = errorParts(javaError);
    expect(parts.message).toContain("newest Java on this computer");
    expect(parts.hint).toContain("Rescan");
    expect(parts.technical).toBe(javaError.technical);
    expect(parts.kind).toBe("java_too_old");
  });

  it("does not hide anything when the failure is not an AppError", () => {
    // A thrown JS error has no plain-language half, so hiding its text would
    // leave the user with nothing at all.
    const parts = errorParts(new Error("window is not defined"));
    expect(parts.message).toBe("window is not defined");
    expect(parts.technical).toBe("window is not defined");
    expect(parts.hint).toBeNull();
    expect(parts.kind).toBe("unknown");
  });
});

describe("ErrorNotice", () => {
  it("leads with the sentence and the fix, not the Rust text", () => {
    render(<ErrorNotice error={javaError} />);

    const alert = screen.getByRole("alert");
    expect(alert).toHaveTextContent("newest Java on this computer");
    expect(alert).toHaveTextContent("Rescan");
    expect(alert).not.toHaveTextContent("the newest found is Java 17");
  });

  it("reveals the technical text on request, and says so to a screen reader", async () => {
    const user = userEvent.setup();
    render(<ErrorNotice error={javaError} />);

    const details = screen.getByRole("button", { name: /details/i });
    expect(details).toHaveAttribute("aria-expanded", "false");

    await user.click(details);

    expect(details).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByText(/the newest found is Java 17/)).toBeInTheDocument();
    // The kind travels too: it is what a maintainer greps for.
    expect(screen.getByText(/java_too_old/)).toBeInTheDocument();
  });

  it("offers no expander when there is nothing extra to show", () => {
    render(<ErrorNotice error={{ kind: "cancelled", message: "Cancelled.", technical: "Cancelled." }} />);
    expect(screen.queryByRole("button", { name: /details/i })).toBeNull();
  });

  it("is reachable by keyboard", async () => {
    const user = userEvent.setup();
    render(<ErrorNotice error={javaError} />);

    await user.tab();
    expect(screen.getByRole("button", { name: /details/i })).toHaveFocus();
  });
});
