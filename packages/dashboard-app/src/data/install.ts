import type { SetupKey } from "@/lib/store";
import type { Install } from "@fig/api-bindings";

type Check = {
  title: string;
  detail: string;
  action: string;
  status: Partial<Record<Install.SetupStatus, string>>;
  alwaysAction?: boolean;
};
export const checks: Record<SetupKey, Check> = {
  dotfiles: {
    title: "zsh integration",
    detail: "Links the CLI and adds hooks to .zshrc and .zprofile. Open a new zsh tab after setup.",
    action: "Set up zsh",
    status: { ready: "CLI and shell hooks installed", missing: "Setup needed" },
  },
  accessibility: {
    title: "Accessibility",
    detail: "Allow Fig in System Settings so it can follow the active terminal window.",
    action: "Open Accessibility settings",
    status: { ready: "Allowed by macOS", missing: "Permission needed" },
  },
  ghosttyInputMethod: {
    title: "Ghostty cursor positioning",
    detail:
      "Optional for Ghostty. Allow Fig Input Method when macOS asks. Keep your usual keyboard layout. To verify placement, type git followed by a space in a Ghostty zsh tab.",
    action: "Set up input method",
    status: {
      ready: "Input method enabled and selected",
      missing: "Input method setup needed",
      approval: "Approval needed in macOS",
      selection: "Approved · finish activation",
    },
    alwaysAction: true,
  },
  loginItem: {
    title: "Login item status",
    detail: "macOS manages approval separately from the Start with macOS preference.",
    action: "Open Login Items",
    status: {
      ready: "Enabled in macOS",
      missing: "Off in macOS",
      approval: "Approval needed in Login Items",
    },
    alwaysAction: true,
  },
};
