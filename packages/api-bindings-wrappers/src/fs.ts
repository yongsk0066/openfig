import { fs as FileSystem } from "@fig/api-bindings";

export const fread = (path: string): Promise<string> =>
  FileSystem.read(path).then((out) => out ?? "");
