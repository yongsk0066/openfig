import { Process } from "../index.js";
import { createErrorInstance } from "@fig/shared/errors";
import logger from "loglevel";
import { cleanOutput, executeCommandTimeout } from "./executeCommand.js";
import { fread } from "./fs.js";

export const LoginShellError = createErrorInstance("LoginShellError");

const DONE_SOURCING_OSC = "\u001b]697;DoneSourcing\u0007";

let etcShells: Promise<string[]> | undefined;

const getShellExecutable = async (
  shellName: string,
  context?: { sessionId?: string; cwd?: string; env?: Record<string, string> },
  signal?: AbortSignal,
) => {
  if (!etcShells) {
    etcShells = fread("/etc/shells").then((shells) =>
      shells
        .split("\n")
        .map((line) => line.trim())
        .filter((line) => line && !line.startsWith("#")),
    );
  }

  try {
    return (
      (await etcShells).find((shell) => shell.includes(shellName)) ??
      (
        await executeCommandTimeout(
          {
            command: "/usr/bin/which",
            args: [shellName],
          },
          5000,
          signal,
          context,
        )
      ).stdout
    );
  } catch (_) {
    return undefined;
  }
};

export const executeLoginShell = async ({
  command,
  executable,
  shell,
  timeout,
  context,
  signal,
}: {
  command: string;
  executable?: string;
  shell?: string;
  timeout?: number;
  context?: { sessionId?: string; cwd?: string; env?: Record<string, string> };
  signal?: AbortSignal;
}): Promise<string> => {
  let exe = executable;
  if (!exe) {
    if (!shell) {
      throw new LoginShellError("Must pass shell or executable");
    }
    exe = await getShellExecutable(shell, context, signal);
    if (!exe) {
      throw new LoginShellError(`Could not find executable for ${shell}`);
    }
  }
  const flags = window.fig.constants?.os === "linux" ? "-lc" : "-lic";

  const process = Process.run({
    executable: exe,
    args: [flags, command],
    terminalSessionId: context ? context.sessionId : window.globalTerminalSessionId,
    workingDirectory: context?.cwd,
    environment: context?.env,
    signal,
    timeout: timeout ?? 5000,
  });

  try {
    logger.info(`About to run login shell command '${command}'`, {
      separateProcess: Boolean(window.f.Process),
      shell: exe,
    });
    const start = performance.now();
    const result = await process.then((output) => {
      if (output.exitCode !== 0) {
        logger.warn(
          `Command ${command} exited with exit code ${output.exitCode}: ${output.stderr}`,
        );
      }
      return cleanOutput(output.stdout);
    });
    const marker = result.lastIndexOf(DONE_SOURCING_OSC);
    const trimmed = marker < 0 ? result : result.slice(marker + DONE_SOURCING_OSC.length);
    const end = performance.now();
    logger.info(`Result of login shell command '${command}'`, {
      result: trimmed,
      time: end - start,
    });
    return trimmed;
  } catch (err) {
    logger.error(`Error running login shell command '${command}'`, { err });
    throw err;
  }
};

export const executeCommand: Fig.ExecuteCommandFunction = (args) => executeCommandTimeout(args);
