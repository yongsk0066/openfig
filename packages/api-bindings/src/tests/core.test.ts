/// <reference types="node" />
import { create, fromBinary, toBinary } from "@bufbuild/protobuf";
import {
  ClientOriginatedMessageSchema,
  GetPlatformInfoRequestSchema,
  GetPlatformInfoResponseSchema,
  ServerOriginatedMessageSchema,
  type ServerOriginatedMessage,
} from "@fig/proto/fig";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { sendMessage } from "../core";
import { sendGetPlatformInfoRequest } from "../requests";
import { bytesToBase64 } from "../utils";

type FetchArgs = [URL, RequestInit];

const requestId = (init: RequestInit): bigint =>
  fromBinary(ClientOriginatedMessageSchema, init.body as Uint8Array).id!;

const encode = (message: Omit<ServerOriginatedMessage, "$typeName" | "$unknown">) =>
  toBinary(ServerOriginatedMessageSchema, create(ServerOriginatedMessageSchema, message));

/** Delivers a message the way the desktop pushes one outside a fetch response. */
const push = (message: Omit<ServerOriginatedMessage, "$typeName" | "$unknown">) =>
  document.dispatchEvent(
    new CustomEvent("FigProtoMessageReceived", { detail: bytesToBase64(encode(message)) }),
  );

const settle = <T>(promise: Promise<T>) =>
  Promise.race([
    promise.then(
      (value) => ({ status: "resolved" as const, value }),
      (error: Error) => ({ status: "rejected" as const, message: error.message }),
    ),
    new Promise<{ status: "pending" }>((resolve) =>
      setTimeout(() => resolve({ status: "pending" }), 50),
    ),
  ]);

describe("sendMessage over api://", () => {
  const fetchMock = vi.fn<(...args: FetchArgs) => Promise<Response>>();

  beforeEach(() => {
    vi.stubGlobal("fig", {
      constants: { supportApiProto: true, apiProtoUrl: "api://localhost/" },
      quiet: true,
    });
    vi.stubGlobal("fetch", fetchMock);
  });

  afterEach(() => {
    fetchMock.mockReset();
    vi.unstubAllGlobals();
  });

  it("resolves with the decoded response", async () => {
    fetchMock.mockImplementation(async (_url, init) => {
      const body = encode({
        id: requestId(init),
        submessage: {
          case: "getPlatformInfoResponse",
          value: create(GetPlatformInfoResponseSchema, { os: 1 }),
        },
      });
      return new Response(body, { status: 200 });
    });

    const result = await settle(sendGetPlatformInfoRequest({}));
    expect(result).toMatchObject({ status: "resolved", value: { os: 1 } });

    const [url, init] = fetchMock.mock.calls[0];
    expect(url.pathname).toBe("/getPlatformInfoRequest");
    expect(init.method).toBe("POST");
  });

  it.each([
    [405, "Method Not Allowed"],
    [415, "Unsupported Media Type"],
    [500, "Internal Server Error"],
  ])("rejects instead of hanging on HTTP %i", async (status, statusText) => {
    fetchMock.mockResolvedValue(new Response("not a protobuf", { status, statusText }));

    const result = await settle(sendGetPlatformInfoRequest({}));
    expect(result.status).toBe("rejected");
    expect(result).toMatchObject({ message: expect.stringContaining(String(status)) });
  });

  it("rejects when the transport fails", async () => {
    fetchMock.mockRejectedValue(new TypeError("Load failed"));
    const result = await settle(sendGetPlatformInfoRequest({}));
    expect(result).toEqual({ status: "rejected", message: expect.stringContaining("Load failed") });
  });

  it("rejects when a 200 body is not a ServerOriginatedMessage", async () => {
    fetchMock.mockResolvedValue(new Response(new Uint8Array([0xff, 0xff, 0xff]), { status: 200 }));
    const result = await settle(sendGetPlatformInfoRequest({}));
    expect(result.status).toBe("rejected");
  });

  it("forgets the handler of a failed request", async () => {
    let id = 0n;
    fetchMock.mockImplementation(async (_url, init) => {
      id = requestId(init);
      return new Response("", { status: 405 });
    });
    const handler = vi.fn();
    sendMessage(
      { case: "getPlatformInfoRequest", value: create(GetPlatformInfoRequestSchema) },
      handler,
    );
    await vi.waitFor(() => expect(handler).toHaveBeenCalledTimes(1));
    expect(handler.mock.calls[0][0]).toMatchObject({ case: "error" });

    // A late response for the same id must not reach the handler again.
    push({ id, submessage: { case: "success", value: true } });
    expect(handler).toHaveBeenCalledTimes(1);
  });

  it("does not call a handler twice when the handler itself throws", async () => {
    fetchMock.mockImplementation(
      async (_url, init) =>
        new Response(
          encode({
            id: requestId(init),
            submessage: { case: "success", value: true },
          }),
          { status: 200 },
        ),
    );
    // The throw still surfaces as an unhandled rejection (as before); keep it out of the run.
    const onUnhandled = vi.fn();
    process.on("unhandledRejection", onUnhandled);
    const handler = vi.fn(() => {
      throw new Error("handler bug");
    });
    sendMessage(
      { case: "getPlatformInfoRequest", value: create(GetPlatformInfoRequestSchema) },
      handler,
    );
    await vi.waitFor(() => expect(handler).toHaveBeenCalledTimes(1));
    await new Promise((resolve) => setTimeout(resolve, 10));
    process.off("unhandledRejection", onUnhandled);
    expect(handler).toHaveBeenCalledTimes(1);
  });
});
