import { act } from "react";
import { createRoot } from "react-dom/client";
import { expect, it, vi } from "vitest";
import { useDynamicResizeObserver } from "./helpers";
it("observes replacement nodes, rounds sizes and ignores callbacks after cleanup", async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  const callbacks: ResizeObserverCallback[] = [];
  const observers: { observe: ReturnType<typeof vi.fn>; disconnect: ReturnType<typeof vi.fn> }[] =
    [];
  vi.stubGlobal(
    "ResizeObserver",
    class {
      observe = vi.fn();
      disconnect = vi.fn();
      constructor(callback: ResizeObserverCallback) {
        callbacks.push(callback);
        observers.push(this);
      }
    },
  );
  const host = document.createElement("div");
  const root = createRoot(host);
  const resized = vi.fn();
  function Probe({ tag }: { tag: string }) {
    const { ref, width, height } = useDynamicResizeObserver({ onResize: resized });
    return (
      <div key={tag} ref={ref}>
        {width},{height}
      </div>
    );
  }
  try {
    await act(async () => root.render(<Probe tag="first" />));
    const emit = (index: number, width: number) =>
      callbacks[index](
        [{ contentRect: { width, height: 19.7 } } as ResizeObserverEntry],
        {} as ResizeObserver,
      );
    await act(async () => emit(0, 40.2));
    expect(host.textContent).toBe("40,20");
    const count = resized.mock.calls.length;
    await act(async () => emit(0, 40.3));
    expect(resized).toHaveBeenCalledTimes(count);
    await act(async () => root.render(<Probe tag="second" />));
    expect(observers[0].disconnect).toHaveBeenCalledOnce();
    await act(async () => emit(0, 99));
    expect(host.textContent).toBe("40,20");
    await act(async () => emit(1, 50));
    expect(host.textContent).toBe("50,20");
    await act(async () => root.unmount());
    expect(observers[1].disconnect).toHaveBeenCalledOnce();
  } finally {
    vi.unstubAllGlobals();
  }
});

it("measures a hidden webview commit without waiting for ResizeObserver and never replays stale zero", async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  vi.stubGlobal(
    "ResizeObserver",
    class {
      observe() {}
      disconnect() {}
    },
  );
  let size = { width: 0, height: 0 };
  const measured = vi
    .spyOn(Element.prototype, "getBoundingClientRect")
    .mockImplementation(() => size as DOMRect);
  const host = document.createElement("div");
  const root = createRoot(host);
  const first = vi.fn();
  const visible = vi.fn();
  function Probe({ onResize }: { onResize: typeof first }) {
    const { ref } = useDynamicResizeObserver({ onResize, measureOnCommit: true });
    return <div ref={ref} />;
  }
  try {
    await act(async () => root.render(<Probe onResize={first} />));
    expect(first).toHaveBeenLastCalledWith({ width: 0, height: 0 });
    size = { width: 380, height: 220 };
    await act(async () => root.render(<Probe onResize={visible} />));
    expect(visible.mock.calls).toEqual([[{ width: 380, height: 220 }]]);
    await act(async () => root.render(<Probe onResize={visible} />));
    expect(visible).toHaveBeenCalledOnce();
    size = { width: 380, height: 260 };
    await act(async () => root.render(<Probe onResize={visible} />));
    expect(visible).toHaveBeenLastCalledWith({ width: 380, height: 260 });
    await act(async () => root.unmount());
  } finally {
    measured.mockRestore();
    vi.unstubAllGlobals();
  }
});
