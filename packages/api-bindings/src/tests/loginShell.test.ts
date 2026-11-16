import { beforeEach, describe, expect, it, vi } from "vitest";
import { Process } from "../index";
import { executeLoginShell } from "../wrappers/executeCommandWrappers";
vi.mock("../process", () => ({ run: vi.fn() }));
beforeEach(() => {
  window.fig = { constants: { os: "macos" } } as never;
  window.f = {} as never;
});
describe("login shell output and execution context", () => {
  it.each(["plain output", "\u001b]697;DoneSourcing\u0007plain output"])(
    "preserves output: %s",
    async (stdout) => {
      vi.mocked(Process.run).mockResolvedValue({ stdout, stderr: "", exitCode: 0 } as never);
      const context = { sessionId: "session-A", cwd: "/repo", env: { CUSTOM: "yes" } };
      const signal = new AbortController().signal;
      expect(
        await executeLoginShell({ command: "echo test", executable: "/bin/zsh", context, signal }),
      ).toBe("plain output");
      expect(Process.run).toHaveBeenLastCalledWith(
        expect.objectContaining({
          terminalSessionId: "session-A",
          workingDirectory: "/repo",
          environment: { CUSTOM: "yes" },
          signal,
        }),
      );
    },
  );
});
