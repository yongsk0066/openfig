declare global {
  interface Window {
    figRuntime?: {
      nonce: string;
      epoch?: string;
      ready: Promise<string>;
      hello(): void;
      fatal(): void;
      bind(nonce: string, epoch: string): void;
    };
    figReceiveMessage?: (raw: string) => void;
  }
}
import {
  type ServerOriginatedMessage,
  type ClientOriginatedMessage,
  ClientOriginatedMessageSchema,
  ServerOriginatedMessageSchema,
} from "@fig/proto/fig";

import { b64ToBytes, bytesToBase64 } from "./utils.js";
import { create, fromBinary, toBinary } from "@bufbuild/protobuf";

interface GlobalAPIError {
  error: string;
}

const FigGlobalErrorOccurred = "FigGlobalErrorOccurred";
const FigProtoMessageReceived = "FigProtoMessageReceived";

type shouldKeepListening = boolean;

export type APIResponseHandler = (
  response: ServerOriginatedMessage["submessage"],
) => shouldKeepListening | void;

let messageId = 0;
const handlers: Record<string, APIResponseHandler> = {};

export function setHandlerForId(handler: APIResponseHandler, id: string) {
  setupEventListeners();
  handlers[id] = handler;
}

const receivedMessage = (response: ServerOriginatedMessage): void => {
  if (response.id === undefined) {
    return;
  }

  const id = response.id.toString();
  const handler = handlers[id];

  if (!handler) {
    return;
  }

  const keepListeningOnID = handlers[id](response.submessage);

  if (!keepListeningOnID) {
    delete handlers[id];
  }
};

const receive = (raw: string) => {
  const bytes = b64ToBytes(raw);
  const message = fromBinary(ServerOriginatedMessageSchema, bytes);
  receivedMessage(message);
};

// The request never produced a response: settle its handler with an error so the promise
// wrapped around it rejects instead of waiting forever, and forget the handler.
const failRequest = (id: string, err: unknown) => {
  const handler = handlers[id];
  delete handlers[id];
  const message = err instanceof Error ? err.message : String(err);
  if (handler) {
    handler({ case: "error", value: message });
  } else {
    console.error(`[fig] request ${id} failed: ${message}`);
  }
};

export function sendMessage(
  submessage: ClientOriginatedMessage["submessage"],
  handler?: APIResponseHandler,
) {
  setupEventListeners();
  const request: ClientOriginatedMessage = create(ClientOriginatedMessageSchema, {
    id: BigInt((messageId += 1)),
    submessage,
  });

  if (handler && request.id) {
    handlers[request.id.toString()] = handler;
  }

  const buffer = toBinary(ClientOriginatedMessageSchema, request);

  if (window?.fig?.constants?.supportApiProto && window?.fig?.constants?.apiProtoUrl) {
    const url = new URL(window.fig.constants.apiProtoUrl);

    if (typeof request.submessage?.case === "string") {
      url.pathname = `/${request.submessage.case}`;
    } else {
      url.pathname = "/unknown";
    }

    const id = String(request.id);
    void (async () => {
      const epoch = window.figRuntime ? await window.figRuntime.ready : undefined;
      return fetch(url, {
        signal: AbortSignal.timeout(65_000),
        method: "POST",
        headers: {
          "content-type": "application/fig-api",
          ...(epoch ? { "x-fig-view-epoch": epoch } : {}),
        },
        body: buffer,
      });
    })()
      .then(async (res) => {
        // A 405/415 body is plain text; decoding it would yield garbage or throw.
        if (!res.ok) {
          throw new Error(
            `${url.pathname.slice(1)} failed: HTTP ${res.status} ${res.statusText}`.trimEnd(),
          );
        }
        return fromBinary(ServerOriginatedMessageSchema, new Uint8Array(await res.arrayBuffer()));
      })
      // Two-argument `then`: a throw inside a handler is the handler's bug and must not be
      // reported to that same handler as a transport failure.
      .then(receivedMessage, (err: unknown) => failRequest(id, err));
    return;
  }

  // Test-only transport. The desktop app always sets `supportApiProto`, so this is unreachable
  // there; `tests/fig-api/fig-api-mock` uses `javascript_init(false)` and drives the API through
  // exactly this path.
  const postMessage = window?.ipc?.postMessage;
  if (postMessage && typeof postMessage === "function") {
    const b64 = bytesToBase64(buffer);
    postMessage(b64);
    return;
  }

  console.error("Cannot send request. Fig.js is not supported in this browser.");
}

const documents = new WeakSet<Document>();
const setupEventListeners = (): void => {
  if (typeof document === "undefined" || documents.has(document)) return;
  documents.add(document);
  document.addEventListener(FigGlobalErrorOccurred, (event: Event) => {
    const response = (event as CustomEvent).detail as GlobalAPIError;
    console.error(response.error);
  });
  // Native delivery calls directly: decode/handler errors must reach the NACK boundary.
  window.figReceiveMessage = receive;
  document.addEventListener(FigProtoMessageReceived, (event: Event) => {
    receive((event as CustomEvent).detail as string);
  });
};
