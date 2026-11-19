import { Platform } from "@fig/api-bindings";
import { act, useState } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { StoreContext } from "@/context/zustand";
import ListenerContext from "@/context/input";
import { createStore, type Store } from "@/lib/store";
import KeystrokeGroup from "../ui/keystrokeInput";
import { Setting } from "./listItem";

let container: HTMLDivElement;
let root: Root;
let store: Store;
const save = vi.fn(async (_key: string, _value: unknown) => {});
const subscribe = async () => ({ unsubscribe() {} });

beforeEach(() => {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  save.mockReset();
  store = createStore({
    settings: { current: async () => ({}), set: save, didChange: { subscribe } },
    state: { current: async () => ({}), set: async () => {}, didChange: { subscribe } },
    install: {
      install: async () => {},
      getStatus: async () => "ready",
      installStatus: { subscribe, onLoginItemChange: subscribe },
    },
    platform: {
      getPlatformInfo: async () => ({
        $typeName: "fig.GetPlatformInfoResponse",
        os: Platform.Os.MACOS,
      }),
    },
  });
  store.setState({ settings: {} });
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
});
afterEach(async () => {
  await act(async () => root.unmount());
  store.dispose();
  container.remove();
});

it("reflects external setting changes and a rejected optimistic write", async () => {
  await act(async () =>
    root.render(
      <StoreContext value={store}>
        <Setting data={{ id: "enabled", type: "boolean", title: "Enabled", default: false }} />
      </StoreContext>,
    ),
  );
  const toggle = container.querySelector<HTMLButtonElement>('[role="switch"]')!;
  expect(toggle.getAttribute("aria-checked")).toBe("false");
  await act(async () => store.setState({ settings: { enabled: true } }));
  expect(toggle.getAttribute("aria-checked")).toBe("true");
  let rejectSave!: (reason: Error) => void;
  save.mockImplementationOnce(
    () =>
      new Promise<void>((_resolve, reject) => {
        rejectSave = reject;
      }),
  );
  await act(async () => toggle.click());
  expect(toggle.getAttribute("aria-checked")).toBe("false");
  expect(save).toHaveBeenCalledWith("enabled", false);
  await act(async () => rejectSave(new Error("save failed")));
  expect(toggle.getAttribute("aria-checked")).toBe("true");
  expect(store.getState().error).toContain("save failed");
});

function KeyboardSettings() {
  const [listening, setListening] = useState<string | null>(null);
  return (
    <ListenerContext value={{ listening, setListening }}>
      <KeystrokeGroup id="autocomplete.accept" defaults={[]} />
      <KeystrokeGroup id="autocomplete.next" defaults={[]} />
    </ListenerContext>
  );
}

it("resets cancelled key input and keeps only the active editor listening", async () => {
  await act(async () =>
    root.render(
      <StoreContext value={store}>
        <KeyboardSettings />
      </StoreContext>,
    ),
  );
  await act(async () => container.querySelector<HTMLButtonElement>("button")!.click());
  await act(async () =>
    document.dispatchEvent(
      new KeyboardEvent("keydown", {
        key: "k",
        keyCode: 75,
        metaKey: true,
        bubbles: true,
        cancelable: true,
      }),
    ),
  );
  expect(container.textContent).toContain("combination is invalid");
  // Opening the second editor closes and unmounts the first editor's input/listener.
  await act(async () => Array.from(container.querySelectorAll("button")).at(-1)!.click());
  expect(container.textContent).not.toContain("combination is invalid");
  expect(container.textContent).toContain("press keys");
  await act(async () =>
    document.dispatchEvent(
      new KeyboardEvent("keydown", {
        key: "n",
        keyCode: 78,
        ctrlKey: true,
        bubbles: true,
        cancelable: true,
      }),
    ),
  );
  expect(container.textContent).not.toContain("combination is invalid");
  await act(async () => Array.from(container.querySelectorAll("button")).at(-1)!.click());
  expect(save).toHaveBeenCalledExactlyOnceWith("autocomplete.keybindings.control+n", "next");
  const event = new KeyboardEvent("keydown", {
    key: "x",
    keyCode: 88,
    bubbles: true,
    cancelable: true,
  });
  document.dispatchEvent(event);
  expect(event.defaultPrevented).toBe(false);
  await act(async () => container.querySelector<HTMLButtonElement>("button")!.click());
  expect(container.textContent).toContain("press keys");
  expect(container.textContent).not.toContain("combination is invalid");
});
