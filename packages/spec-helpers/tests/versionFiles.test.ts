import { expect, it } from "vitest";
import { createVersionedSpec } from "../src/versions";

it("selects version files without changing shared version order", async () => {
  const files = ["2.0.0", "1.0.0", "1.5.0"];
  const load = createVersionedSpec("tool", files);
  expect(typeof load).toBe("function");
  if (typeof load !== "function") throw new TypeError("Expected a versioned loader");
  expect(await Promise.resolve(load("1.6.0"))).toEqual({
    versionedSpecPath: "tool/1.5.0",
    version: "1.6.0",
  });
  expect(await Promise.resolve(load())).toEqual({
    versionedSpecPath: "tool/2.0.0",
    version: undefined,
  });
  expect(files).toEqual(["2.0.0", "1.0.0", "1.5.0"]);
});
