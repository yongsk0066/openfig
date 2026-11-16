import { SETTINGS, type SettingsMap } from "@fig/shared/settings";
export { SETTINGS, type SettingsMap } from "@fig/shared/settings";
let settings: SettingsMap = {};

export const updateSettings = (newSettings: SettingsMap) => {
  settings = newSettings;
};

export const getSetting = <T = unknown>(key: SETTINGS, defaultValue?: T): T =>
  (settings[key] ?? defaultValue) as T;

export const getSettings = () => settings;

export function isInDevMode(): boolean {
  return Boolean(getSetting(SETTINGS.DEV_MODE)) || Boolean(getSetting(SETTINGS.DEV_MODE_NPM));
}
