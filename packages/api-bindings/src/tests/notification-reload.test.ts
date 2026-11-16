import { create, fromBinary, toBinary } from "@bufbuild/protobuf";
import {
  ClientOriginatedMessageSchema,
  NotificationSchema,
  NotificationType,
  ServerOriginatedMessageSchema,
} from "@fig/proto/fig";
import { afterEach, expect, it, vi } from "vitest";
import { bytesToBase64 } from "../utils";

afterEach(() => vi.unstubAllGlobals());

it("finishes clearing the old page's subscriptions before opening the new ones", async () => {
  vi.resetModules();
  // A fresh document models a WebView reload (including its event listeners).
  vi.stubGlobal("document", document.implementation.createHTMLDocument());
  vi.stubGlobal("fig", {
    constants: { supportApiProto: true, apiProtoUrl: "api://localhost/" },
    quiet: true,
  });

  let finishReset!: () => void;
  let channel: bigint | undefined;
  const reset = new Promise<void>((resolve) => (finishReset = resolve));
  const fetchMock = vi.fn(async (_url: URL, init: RequestInit) => {
    const { id, submessage } = fromBinary(ClientOriginatedMessageSchema, init.body as Uint8Array);
    if (submessage?.case !== "notificationRequest") throw new Error("unexpected request");
    if (submessage.value.subscribe === false) {
      // The native protocol dispatches each fetch on its own task. Make the reset run last,
      // as it can when WebKit reloads while the desktop is under load.
      await reset;
      channel = undefined;
    } else {
      channel = id;
    }
    return new Response(
      toBinary(
        ServerOriginatedMessageSchema,
        create(ServerOriginatedMessageSchema, { id, submessage: { case: "success", value: true } }),
      ),
    );
  });
  vi.stubGlobal("fetch", fetchMock);

  const { subscribeNotification } = await import("../notifications");
  const handler = vi.fn();
  const subscribed = subscribeNotification(
    { type: NotificationType.NOTIFY_ON_EDITBUFFFER_CHANGE },
    handler,
  );
  await Promise.resolve();
  finishReset();
  await subscribed;

  expect(
    channel,
    "a late reset must not silently erase the new edit-buffer subscription",
  ).toBeDefined();
  document.dispatchEvent(
    new CustomEvent("FigProtoMessageReceived", {
      detail: bytesToBase64(
        toBinary(
          ServerOriginatedMessageSchema,
          create(ServerOriginatedMessageSchema, {
            id: channel,
            submessage: { case: "notification", value: create(NotificationSchema) },
          }),
        ),
      ),
    }),
  );
  expect(handler).toHaveBeenCalledTimes(1);
});

it("retries a failed reset instead of leaving all future subscribers rejected", async () => {
  vi.resetModules();
  vi.stubGlobal("document", document.implementation.createHTMLDocument());
  vi.stubGlobal("fig", {
    constants: { supportApiProto: true, apiProtoUrl: "api://localhost/" },
    quiet: true,
  });
  let resets = 0;
  let subscriptions = 0;
  vi.stubGlobal(
    "fetch",
    vi.fn(async (_url: URL, init: RequestInit) => {
      const { id, submessage } = fromBinary(ClientOriginatedMessageSchema, init.body as Uint8Array);
      if (submessage?.case !== "notificationRequest") throw new Error("unexpected request");
      if (submessage.value.subscribe === false) {
        if (++resets === 1) throw new TypeError("Load failed");
      } else {
        subscriptions++;
      }
      return new Response(
        toBinary(
          ServerOriginatedMessageSchema,
          create(ServerOriginatedMessageSchema, {
            id,
            submessage: { case: "success", value: true },
          }),
        ),
      );
    }),
  );
  const { subscribeNotification } = await import("../notifications");
  const request = { type: NotificationType.NOTIFY_ON_EDITBUFFFER_CHANGE };
  await expect(subscribeNotification(request, vi.fn())).rejects.toThrow("Load failed");
  await subscribeNotification(request, vi.fn());
  expect(resets).toBe(2);
  expect(subscriptions).toBe(1);
});
