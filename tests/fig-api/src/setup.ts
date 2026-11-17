import childProcess from "node:child_process";
import { tempDir } from "./util.js";

// Set Fig JS bindings to log less
window.fig = { constants: undefined, quiet: true } as unknown as typeof fig;

// Directory requests exercise the same isolated worker as the desktop.
childProcess.execFileSync("cargo", [
  "build",
  "--locked",
  "-p",
  "fig-api-mock",
  "-p",
  "fig_desktop",
  "--bin",
  "fig-api-mock",
  "--bin",
  "fig_io_worker",
  "-q",
]);

// Init constants
eval(
  childProcess
    .execSync("cargo run -p fig-api-mock -- init", {
      stdio: "pipe",
    })
    .toString(),
);

// Set up IPC
window.ipc = {
  postMessage: (message: string) => {
    // `execSync` hands back a Buffer; the real transport is `evaluate_script`, which always
    // delivers the base64 payload as a string. Send what the app sends.
    const detail = childProcess
      .execSync(`cargo run -p fig-api-mock -q -- request --cwd ${tempDir} ${message}`, {
        stdio: "pipe",
      })
      .toString()
      .trim();
    document.dispatchEvent(new CustomEvent("FigProtoMessageReceived", { detail }));
  },
};
