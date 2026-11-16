import { act } from "react";
import { createRoot } from "react-dom/client";
import { expect, it, vi } from "vitest";
import ErrorBoundary from "./ErrorBoundary";
const Crash = () => {
  throw new Error("injected render failure");
};
it("reports a render crash to the native recovery owner without scheduling a page reload", async () => {
  vi.useFakeTimers();
  const fatal = vi.fn();
  const previous = window.figRuntime;
  window.figRuntime = {
    nonce: "test",
    epoch: "3",
    ready: Promise.resolve("3"),
    bind: vi.fn(),
    hello: vi.fn(),
    fatal,
  };
  const error = vi.spyOn(console, "error").mockImplementation(() => {});
  const root = createRoot(document.createElement("div"));
  try {
    await act(async () =>
      root.render(
        <ErrorBoundary>
          <Crash />
        </ErrorBoundary>,
      ),
    );
    expect(fatal).toHaveBeenCalledOnce();
    expect(vi.getTimerCount()).toBe(0);
  } finally {
    await act(async () => root.unmount());
    window.figRuntime = previous;
    error.mockRestore();
    vi.useRealTimers();
  }
});
