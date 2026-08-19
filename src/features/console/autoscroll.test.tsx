import { act, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useEffect, useMemo, useRef, useState } from "react";
import { describe, expect, it, vi } from "vitest";

import { BOTTOM_THRESHOLD_PX, isAtBottom } from "./useConsole";

describe("isAtBottom", () => {
  it("counts a couple of pixels of slack as the bottom", () => {
    // Exactly parked at the end.
    expect(isAtBottom({ scrollHeight: 1000, scrollTop: 800, clientHeight: 200 })).toBe(true);
    // A rounding error or a partial last line is still the bottom.
    expect(isAtBottom({ scrollHeight: 1000, scrollTop: 790, clientHeight: 200 })).toBe(true);
    expect(
      isAtBottom({ scrollHeight: 1000, scrollTop: 800 - BOTTOM_THRESHOLD_PX, clientHeight: 200 }),
    ).toBe(true);
  });

  it("counts a deliberate scroll up as away from the bottom", () => {
    // One wheel notch is far more than the slack.
    expect(isAtBottom({ scrollHeight: 1000, scrollTop: 700, clientHeight: 200 })).toBe(false);
    expect(isAtBottom({ scrollHeight: 1000, scrollTop: 0, clientHeight: 200 })).toBe(false);
  });

  it("treats a container with nothing to scroll as at the bottom", () => {
    expect(isAtBottom({ scrollHeight: 100, scrollTop: 0, clientHeight: 100 })).toBe(true);
  });
});

/**
 * The console's scrolling rule, in the shape the component uses it: an effect
 * that pins the view to the bottom while autoscroll is on, and a scroll handler
 * that releases it — with the component's own jump excluded, or a server
 * printing continuously would re-arm autoscroll a frame after the user scrolled
 * away and the view would snap back for ever.
 */
function Console({ lines, guard }: { lines: string[]; guard: boolean }) {
  const [autoscroll, setAutoscroll] = useState(true);
  const scroller = useRef<HTMLDivElement>(null);
  const selfScrolling = useRef(false);
  const visible = useMemo(() => lines, [lines]);

  useEffect(() => {
    const element = scroller.current;
    if (!autoscroll || !element) return;
    if (Math.abs(element.scrollTop - element.scrollHeight) <= 1) return;

    selfScrolling.current = true;
    // The jump itself. The browser fires the matching `scroll` event on a later
    // frame, which the tests deliver by hand — that delay is what let the old
    // code mistake its own jump for the user coming back to the bottom.
    element.scrollTop = element.scrollHeight;

    const frame = requestAnimationFrame(() => {
      selfScrolling.current = false;
    });
    return () => cancelAnimationFrame(frame);
  }, [visible, autoscroll]);

  return (
    <>
      <div
        data-testid="scroller"
        role="log"
        tabIndex={0}
        ref={scroller}
        onScroll={(event) => {
          if (guard && selfScrolling.current) {
            selfScrolling.current = false;
            return;
          }
          const atBottom = isAtBottom(event.currentTarget);
          if (atBottom !== autoscroll) setAutoscroll(atBottom);
        }}
      >
        {visible.map((line) => (
          <div key={line}>{line}</div>
        ))}
      </div>
      <output data-testid="state">{autoscroll ? "following" : "released"}</output>
    </>
  );
}

/** jsdom reports zeroes for layout, so the geometry is supplied directly. */
function measure(element: HTMLElement, { scrollHeight = 1000, clientHeight = 200 } = {}) {
  Object.defineProperty(element, "scrollHeight", { value: scrollHeight, configurable: true });
  Object.defineProperty(element, "clientHeight", { value: clientHeight, configurable: true });
}

function scrollTo(element: HTMLElement, top: number) {
  element.scrollTop = top;
  fireEvent.scroll(element);
}

/** Lets the frame that clears the self-scroll guard run. */
async function nextFrame() {
  await act(async () => {
    await new Promise((resolve) => requestAnimationFrame(() => resolve(null)));
  });
}

describe("console autoscroll", () => {
  it("releases when the user scrolls up", async () => {
    render(<Console lines={["a", "b"]} guard />);
    const scroller = screen.getByTestId("scroller");
    measure(scroller);
    await nextFrame();

    expect(screen.getByTestId("state")).toHaveTextContent("following");
    scrollTo(scroller, 400);
    expect(screen.getByTestId("state")).toHaveTextContent("released");
  });

  it("re-arms when the user scrolls back to the bottom", async () => {
    render(<Console lines={["a", "b"]} guard />);
    const scroller = screen.getByTestId("scroller");
    measure(scroller);
    await nextFrame();

    scrollTo(scroller, 400);
    expect(screen.getByTestId("state")).toHaveTextContent("released");

    scrollTo(scroller, 800);
    expect(screen.getByTestId("state")).toHaveTextContent("following");
  });

  it("stays released while output keeps arriving", async () => {
    // The reported symptom, and the reason the guard exists: on a busy server a
    // batch lands every frame and each one jumps the view to the bottom. The
    // event for that jump arrives after the user has already scrolled away, and
    // it must not count as them coming back.
    const { rerender } = render(<Console lines={["a"]} guard />);
    const scroller = screen.getByTestId("scroller");
    measure(scroller);
    await nextFrame();

    // A batch lands: the component jumps to the bottom, and the browser will
    // deliver that scroll event a moment later.
    scroller.scrollTop = 500;
    rerender(<Console lines={["a", "batch 1"]} guard />);
    expect(scroller.scrollTop).toBe(1000);

    // The user scrolls up before that event arrives — the ordering that made
    // the old code impossible to scroll away from.
    scrollTo(scroller, 1000); // the app's own jump, arriving late
    expect(screen.getByTestId("state")).toHaveTextContent("following");
    scrollTo(scroller, 300); // and now the user
    expect(screen.getByTestId("state")).toHaveTextContent("released");

    // Every batch after that leaves the view where the user put it.
    for (let batch = 2; batch < 7; batch += 1) {
      rerender(<Console lines={["a", `batch ${batch}`]} guard />);
    }
    expect(screen.getByTestId("state")).toHaveTextContent("released");
    expect(scroller.scrollTop).toBe(300);

    // Going back to the bottom by hand re-arms it.
    scrollTo(scroller, 800);
    expect(screen.getByTestId("state")).toHaveTextContent("following");
  });

  it("without the guard, its own jump re-arms it — which is the bug", async () => {
    // Kept as a test so the guard cannot be quietly removed: this is what the
    // console did before, and why scrolling up was impossible on a busy server.
    render(<Console lines={["a"]} guard={false} />);
    const scroller = screen.getByTestId("scroller");
    measure(scroller);
    await nextFrame();

    scrollTo(scroller, 300);
    expect(screen.getByTestId("state")).toHaveTextContent("released");

    // The same late event, with nothing to tell it apart from a user scroll.
    scrollTo(scroller, 1000);
    expect(screen.getByTestId("state")).toHaveTextContent("following");
  });

  it("is reachable and scrollable from the keyboard", async () => {
    const user = userEvent.setup();
    render(<Console lines={["a", "b"]} guard />);
    const scroller = screen.getByTestId("scroller");
    measure(scroller);
    await nextFrame();

    await user.tab();
    expect(scroller).toHaveFocus();
    expect(scroller).toHaveAttribute("role", "log");

    // jsdom does not implement scrolling for key presses, so what is checked is
    // that the region takes focus and that a scroll from any source — wheel,
    // trackpad, scrollbar drag or PageUp — runs the same handler.
    const handler = vi.fn();
    scroller.addEventListener("scroll", handler);
    await user.keyboard("{PageUp}");
    scrollTo(scroller, 0);
    expect(handler).toHaveBeenCalled();
    expect(screen.getByTestId("state")).toHaveTextContent("released");
  });
});
