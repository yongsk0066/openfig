import {
  type Notification,
  type ServerOriginatedMessage,
  type NotificationRequest,
  NotificationRequestSchema,
  NotificationType,
} from "@fig/proto/fig";

import { sendMessage } from "./core.js";
import { create } from "@bufbuild/protobuf";

export type NotificationResponse = {
  unsubscribe: boolean;
};

export type NotificationHandler = (notification: Notification) => NotificationResponse | undefined;
export interface Subscription {
  unsubscribe(): void;
}

const handlers: Partial<Record<NotificationType, NotificationHandler[]>> = {};

/**
 * The request that opened — or is still opening — the desktop-side subscription for a type.
 * Every subscriber of the type waits for it; a failed one is forgotten, so the next subscriber
 * asks again instead of attaching to a subscription that does not exist.
 */
const subscriptions: Partial<Record<NotificationType, Promise<void>>> = {};

export function _unsubscribe(type: NotificationType, handler?: NotificationHandler) {
  if (handler && handlers[type] !== undefined) {
    handlers[type] = (handlers[type] ?? []).filter((x) => x !== handler);
  }
}

const openSubscription = (
  request: Omit<NotificationRequest, "$typeName">,
  type: NotificationType,
): Promise<void> =>
  new Promise<void>((resolve, reject) => {
    handlers[type] = [];

    const fail = (error: Error) => {
      delete subscriptions[type];
      delete handlers[type];
      reject(error);
    };

    let handlersToRemove: NotificationHandler[] | undefined;
    sendMessage(
      {
        case: "notificationRequest",
        value: create(NotificationRequestSchema, { ...request, subscribe: true }),
      },
      (response: ServerOriginatedMessage["submessage"]) => {
        switch (response?.case) {
          case "notification":
            if (!handlers[type]) {
              return false;
            }

            // call handlers and remove any that have unsubscribed (by returning false)
            handlersToRemove = handlers[type]?.filter((existingHandler) => {
              const res = existingHandler(response.value);
              return Boolean(res?.unsubscribe);
            });

            handlers[type] = handlers[type]?.filter(
              (existingHandler) => !handlersToRemove?.includes(existingHandler),
            );

            return true;
          case "success":
            resolve();
            return true;
          case "error":
            fail(new Error(response.value));
            break;
          default:
            fail(new Error("Not a notification"));
            break;
        }

        return false;
      },
    );
  });

export function _subscribe(
  request: Omit<NotificationRequest, "$typeName">,
  handler: NotificationHandler,
): Promise<Subscription> | undefined {
  const { type } = request;
  if (!type) {
    return Promise.reject(new Error("NotificationRequest type must be defined."));
  }

  subscriptions[type] ??= openSubscription(request, type);
  return subscriptions[type].then(() => {
    // A registration of its own, so unsubscribing twice, or after another subscription
    // passed the same function, removes exactly this one.
    const registration: NotificationHandler = (notification) => handler(notification);
    handlers[type] = [...(handlers[type] ?? []), registration];
    return { unsubscribe: () => _unsubscribe(type, registration) };
  });
}

const unsubscribeFromAll = () => {
  sendMessage({
    case: "notificationRequest",
    value: create(NotificationRequestSchema, {
      subscribe: false,
      type: NotificationType.ALL,
    }),
  });
};

if (!window?.fig?.quiet) {
  console.log("[fig] unsubscribing any existing notifications...");
}
unsubscribeFromAll();
