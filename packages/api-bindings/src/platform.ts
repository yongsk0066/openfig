import { GetPlatformInfoResponse } from "@fig/proto/fig";
import { sendGetPlatformInfoRequest } from "./requests.js";
import { AppBundleType, DesktopEnvironment, DisplayServerProtocol, Os } from "@fig/proto/fig";

export { AppBundleType, DesktopEnvironment, DisplayServerProtocol, Os };

export function getPlatformInfo(): Promise<GetPlatformInfoResponse> {
  return sendGetPlatformInfoRequest({});
}
