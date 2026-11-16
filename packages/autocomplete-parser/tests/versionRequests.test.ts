import { expect, it, vi } from "vitest";
import {
  createSpecLoader,
  createSpecLoaderCaches,
  type SpecFileImport,
  type SpecLoaderPorts,
} from "../src/specLoader";
const spec: SpecFileImport = { default: () => ({ name: "tool" }) };
const ports = (executeCommand: Fig.ExecuteCommandFunction): SpecLoaderPorts => ({
  executeCommand,
  fread: vi.fn(),
  isInDevMode: () => false,
  importSource: vi.fn(),
  importModule: vi.fn(),
  readJson: vi.fn(),
});
it("shares concurrent version requests across input loaders without sharing abort ownership", async () => {
  const caches = createSpecLoaderCaches();
  let finish!: (value: Fig.ExecuteCommandOutput) => void;
  const shared = vi.fn(
    () =>
      new Promise<Fig.ExecuteCommandOutput>((resolve) => {
        finish = resolve;
      }),
  );
  const cancelledInput = vi.fn(async () => {
    throw new Error("aborted input");
  });
  const a = createSpecLoader({ ...ports(cancelledInput), executeVersionCommand: shared }, caches);
  const b = createSpecLoader({ ...ports(cancelledInput), executeVersionCommand: shared }, caches);
  const requests = [a.getVersionFromFullFile(spec, "tool"), b.getVersionFromFullFile(spec, "tool")];
  expect(shared).toHaveBeenCalledOnce();
  finish({ stdout: "1.2.3", stderr: "", status: 0 });
  expect(await Promise.all(requests)).toEqual(["1.2.3", "1.2.3"]);
  expect(cancelledInput).not.toHaveBeenCalled();
  expect(await b.getVersionFromFullFile(spec, "tool")).toBe("1.2.3");
  expect(shared).toHaveBeenCalledOnce();
});
it("retries failed version reads and bypasses the cache in development", async () => {
  const execute = vi
    .fn<Fig.ExecuteCommandFunction>()
    .mockRejectedValueOnce(new Error("missing"))
    .mockResolvedValue({ stdout: "2.0.0", stderr: "", status: 0 });
  const loader = createSpecLoader(ports(execute));
  expect(await loader.getVersionFromFullFile(spec, "tool")).toBeUndefined();
  expect(await loader.getVersionFromFullFile(spec, "tool")).toBe("2.0.0");
  const dev = createSpecLoader({ ...ports(execute), isInDevMode: () => true });
  await dev.getVersionFromFullFile(spec, "tool");
  await dev.getVersionFromFullFile(spec, "tool");
  expect(execute).toHaveBeenCalledTimes(4);
});
