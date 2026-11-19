import {
  InstallAction,
  InstallComponent,
  type InstallResponse,
  InstallResponse_InstallationStatus,
  NotificationType,
  Result_Result,
} from "@fig/proto/fig";

import { subscribe as subscribeEvent } from "./event.js";
import { sendInstallRequest } from "./requests.js";
import { type NotificationResponse, subscribeNotification } from "./notifications.js";

export type Component =
  | "ghosttyInputMethod"
  | "loginItem"
  | "dotfiles"
  | "ibus"
  | "accessibility"
  | "desktopEntry"
  | "autostartEntry"
  | "gnomeExtension";

function componentToProto(component: Component) {
  switch (component) {
    case "ghosttyInputMethod":
      return InstallComponent.GHOSTTY_INPUT_METHOD;
    case "loginItem":
      return InstallComponent.LOGIN_ITEM;
    case "dotfiles":
      return InstallComponent.DOTFILES;
    case "ibus":
      return InstallComponent.IBUS;
    case "accessibility":
      return InstallComponent.ACCESSIBILITY;
    case "desktopEntry":
      return InstallComponent.DESKTOP_ENTRY;
    case "autostartEntry":
      return InstallComponent.AUTOSTART_ENTRY;
    case "gnomeExtension":
      return InstallComponent.GNOME_EXTENSION;
    default:
      throw Error("Invalid component");
  }
}

function handleBasicResponse(response: InstallResponse) {
  switch (response.response?.case) {
    case "result":
      if (response.response.value.result === Result_Result.OK) {
        return;
      }
      if (response.response.value.result === Result_Result.ERROR) {
        throw Error(response.response.value.error);
      } else {
        throw Error(`Unexpected result: ${response.response.value.result}`);
      }
    default:
      throw Error(`Unexpected result: ${response.response?.case}`);
  }
}

export async function install(component: Component) {
  const response = await sendInstallRequest({
    action: InstallAction.INSTALL,
    component: componentToProto(component),
  });
  handleBasicResponse(response);
}

export async function uninstall(component: Component) {
  const response = await sendInstallRequest({
    action: InstallAction.UNINSTALL,
    component: componentToProto(component),
  });
  handleBasicResponse(response);
}

export type SetupStatus = "ready" | "missing" | "approval" | "selection" | "unsupported";

export async function getStatus(component: Component): Promise<SetupStatus> {
  const response = await sendInstallRequest({
    action: InstallAction.STATUS,
    component: componentToProto(component),
  });
  switch (response.response?.case) {
    case "installationStatus":
      switch (response.response.value) {
        case InstallResponse_InstallationStatus.INSTALLED:
          return "ready";
        case InstallResponse_InstallationStatus.NOT_INSTALLED:
          return "missing";
        case InstallResponse_InstallationStatus.REQUIRES_APPROVAL:
          return "approval";
        case InstallResponse_InstallationStatus.REQUIRES_SELECTION:
          return "selection";
        case InstallResponse_InstallationStatus.NOT_SUPPORTED:
          return "unsupported";
        default:
          throw Error("Unrecognized installation status");
      }
    case "result":
      throw Error(response.response.value.error ?? "Installation status unavailable");
    default:
      throw Error(`Unexpected result: ${response.response?.case}`);
  }
}

export async function isInstalled(component: Component) {
  return (await getStatus(component)) === "ready";
}

export const installStatus = {
  onLoginItemChange: (handler: () => void) =>
    subscribeEvent("dashboard.loginItemChanged", () => {
      handler();
      return { unsubscribe: false };
    }),

  subscribe: (
    component: "accessibility",
    handler: (isInstalled: boolean) => NotificationResponse | undefined,
  ) => {
    if (component === "accessibility") {
      return subscribeNotification(
        { type: NotificationType.NOTIFY_ON_ACCESSIBILITY_CHANGE },
        (notification) => {
          switch (notification?.type?.case) {
            case "accessibilityChangeNotification":
              return handler(notification.type.value.enabled);
            default:
              break;
          }

          return { unsubscribe: false };
        },
      );
    }
    throw Error("Not implemented");
  },
};
