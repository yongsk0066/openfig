import { create, fromBinary, toBinary } from "@bufbuild/protobuf";
import {
  ClientOriginatedMessageSchema,
  NotificationSchema,
  NotificationType,
  ServerOriginatedMessageSchema,
  type ServerOriginatedMessage,
} from "@fig/proto/fig";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { bytesToBase64 } from "../utils";

let subscribeNotification: typeof import("../notifications").subscribeNotification;

type FetchArgs = [URL, RequestInit];
type Submessage = ServerOriginatedMessage["submessage"];

const encode = (message: Omit<ServerOriginatedMessage, "$typeName" | "$unknown">) =>
  toBinary(ServerOriginatedMessageSchema, create(ServerOriginatedMessageSchema, message));

const push = (message: Omit<ServerOriginatedMessage, "$typeName" | "$unknown">) =>
  document.dispatchEvent(
    new CustomEvent("FigProtoMessageReceived", { detail: bytesToBase64(encode(message)) }),
  );

const notify = (id: bigint) =>
  push({ id, submessage: { case: "notification", value: create(NotificationSchema) } });

const success: Submessage = { case: "success", value: true };

describe("notification subscriptions over api://", () => {
  const fetchMock = vi.fn<(...args: FetchArgs) => Promise<Response>>();
  /** Ids of the notification requests sent, in order. */
  const requests: bigint[] = [];
  /** Answers the next requests with these submessages (an HTTP status for a transport error). */
  const answers: (Submessage | number)[] = [];

  beforeEach(async () => {
    vi.resetModules();
    vi.stubGlobal("document", document.implementation.createHTMLDocument());
    vi.stubGlobal("fig", {
      constants: { supportApiProto: true, apiProtoUrl: "api://localhost/" },
      quiet: true,
    });
    vi.stubGlobal("fetch", fetchMock);
    requests.length = 0;
    answers.length = 0;
    fetchMock.mockImplementation(async (_url, init) => {
      const { id, submessage } = fromBinary(ClientOriginatedMessageSchema, init.body as Uint8Array);
      const clearing =
        submessage?.case === "notificationRequest" && submessage.value.subscribe === false;
      if (!clearing) requests.push(id!);
      const answer = clearing ? success : (answers.shift() ?? success);
      if (typeof answer === "number") return new Response("", { status: answer });
      return new Response(encode({ id, submessage: answer }), { status: 200 });
    });
    ({ subscribeNotification } = await import("../notifications"));
  });

  afterEach(() => {
    fetchMock.mockReset();
    vi.unstubAllGlobals();
  });

  it.each([
    ["an error response", { case: "error", value: "denied" } as Submessage, "denied"],
    ["a transport failure", 405, "405"],
  ])("after %s, the next subscriber sends the request again", async (_name, answer, message) => {
    // Every test uses its own type: subscriptions are module state.
    const type =
      typeof answer === "number"
        ? NotificationType.NOTIFY_ON_PROMPT
        : NotificationType.NOTIFY_ON_FOCUS_CHANGED;
    answers.push(answer);
    const first = vi.fn();
    await expect(subscribeNotification({ type }, first)).rejects.toThrow(message);

    const second = vi.fn();
    await subscribeNotification({ type }, second);
    expect(requests).toHaveLength(2);

    notify(requests[1]);
    expect(second).toHaveBeenCalledTimes(1);
    expect(first).not.toHaveBeenCalled();
  });

  it("a subscriber that arrives while the request is in flight shares its outcome", async () => {
    const type = NotificationType.NOTIFY_ON_LOCATION_CHANGE;
    answers.push({ case: "error", value: "denied" });
    const first = subscribeNotification({ type }, vi.fn());
    const second = subscribeNotification({ type }, vi.fn());

    await expect(first).rejects.toThrow("denied");
    // It must not resolve against a subscription the desktop refused.
    await expect(second).rejects.toThrow("denied");
    expect(requests).toHaveLength(1);
  });

  it("unsubscribe is idempotent and removes only its own registration", async () => {
    const type = NotificationType.NOTIFY_ON_APPLICATION_UPDATE_AVAILABLE;
    const handler = vi.fn();
    const one = await subscribeNotification({ type }, handler);
    const two = await subscribeNotification({ type }, handler);
    expect(requests).toHaveLength(1);

    notify(requests[0]);
    expect(handler).toHaveBeenCalledTimes(2);

    one!.unsubscribe();
    one!.unsubscribe();
    notify(requests[0]);
    expect(handler).toHaveBeenCalledTimes(3);

    two!.unsubscribe();
    notify(requests[0]);
    expect(handler).toHaveBeenCalledTimes(3);
  });
});
