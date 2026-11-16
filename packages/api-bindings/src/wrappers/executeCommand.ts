/**
 * NOTE: this is intended to be separate because executeCommand
 * will often be mocked during testing of functions that call it.
 * If it gets bundled in the same file as the functions that call it
 * vitest is not able to mock it (because of esm restrictions).
 */
import { Process } from "../index.js";
import logger from "loglevel";

export const cleanOutput = (output: string) =>
  output
    .replace(/\r\n/g, "\n") // Replace carriage returns with just a normal return
    // oxlint-disable-next-line no-control-regex
    .replace(/\x1b\[\?25h/g, "") // removes cursor character if present
    .replace(/^\n+/, "") // strips new lines from start of output
    .replace(/\n+$/, ""); // strips new lines from end of output

export const executeCommandTimeout = async (
  input: Fig.ExecuteCommandInput,
  timeout = 5000,
  signal?: AbortSignal,
  context?: { sessionId?: string; cwd?: string; env?: Record<string, string> },
): Promise<Fig.ExecuteCommandOutput> => {
  const command = [input.command, ...input.args].join(" ");
  try {
    logger.info(`About to run shell command '${command}'`);
    const start = performance.now();
    const result = await Process.run({
      executable: input.command,
      args: input.args,
      environment: input.env ?? context?.env,
      workingDirectory: input.cwd ?? context?.cwd,
      terminalSessionId: context ? context.sessionId : window.globalTerminalSessionId,
      timeout: Math.max(timeout, input.timeout ?? 0),
      signal,
    });
    const end = performance.now();
    logger.info(`Result of shell command '${command}'`, {
      result,
      time: end - start,
    });

    const cleanStdout = cleanOutput(result.stdout);
    const cleanStderr = cleanOutput(result.stderr);

    if (result.exitCode !== 0) {
      logger.warn(`Command ${command} exited with exit code ${result.exitCode}: ${cleanStderr}`);
    }
    return {
      status: result.exitCode,
      stdout: cleanStdout,
      stderr: cleanStderr,
    };
  } catch (err) {
    if (signal?.aborted) throw signal.reason;
    logger.error(`Error running shell command '${command}'`, { err });
    throw err;
  }
};
