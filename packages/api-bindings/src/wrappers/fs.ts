import { fs as FileSystem } from "../index.js";

export const fread = (path: string): Promise<string> =>
  FileSystem.read(path).then((out) => out ?? "");
