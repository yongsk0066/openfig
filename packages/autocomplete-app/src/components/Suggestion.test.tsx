import { act } from "react";
import { createRoot } from "react-dom/client";
import { expect, it, vi } from "vitest";
import Suggestion from "./Suggestion";

it("inserts a clicked suggestion without stealing focus from the terminal input", async () => {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  window.fig = { constants: { newUriFormat: true } } as typeof window.fig;
  const container = document.createElement("div");
  const terminal = document.createElement("input");
  document.body.append(terminal, container);
  terminal.focus();
  const root = createRoot(container);
  const onClick = vi.fn();
  const suggestion = {
    name: "checkout",
    type: "subcommand" as const,
    icon: "fig://icon?type=command&badge=1",
  };
  try {
    await act(async () =>
      root.render(
        <Suggestion
          suggestion={suggestion}
          onClick={onClick}
          style={{ height: 24 }}
          isActive
          commonPrefix=""
          iconPath="/tmp/"
          searchTerm=""
          iconSize={16}
          fuzzySearchEnabled={false}
        />,
      ),
    );
    const button = container.querySelector("button")!;
    const down = new MouseEvent("mousedown", { bubbles: true, cancelable: true });
    button.dispatchEvent(down);
    expect(down.defaultPrevented).toBe(true);
    expect(document.activeElement).toBe(terminal);
    button.click();
    expect(onClick).toHaveBeenCalledExactlyOnceWith(suggestion);
    expect(button.tabIndex).toBe(-1);
    const icon = button.querySelector("img")!;
    expect(icon.alt).toBe("Icon for command");
    expect(icon.getAttribute("src")).toBe(suggestion.icon);
    expect(button.textContent).toContain("1");
  } finally {
    await act(async () => root.unmount());
    container.remove();
    terminal.remove();
  }
});
