import type { Types } from "@fig/api-bindings";
import type { AliasMap } from "@fig/shell-parser";
export type FigState = {
  buffer: string;
  cursorLocation: number;
  cwd: string | null;
  processUserIsIn: string | null;
  sshContextString: string | null;
  aliases: AliasMap;
  environmentVariables: Record<string, string>;
  shellContext?: Types.ShellContext | undefined;
};

export const initialFigState: FigState = {
  buffer: "",
  cursorLocation: 0,
  cwd: null,
  processUserIsIn: null,
  sshContextString: null,
  aliases: {},
  environmentVariables: {},
  shellContext: undefined,
};
