import { create } from "@bufbuild/protobuf";
import { sendRunProcessRequest, sendCancelProcessRequest } from "./requests.js";
import { DurationSchema, EnvironmentVariableSchema } from "@fig/proto/fig_common";

export async function run({
  executable,
  args,
  environment,
  workingDirectory,
  terminalSessionId,
  timeout,
  signal,
}: {
  executable: string;
  args: string[];
  environment?: Record<string, string | undefined>;
  workingDirectory?: string;
  terminalSessionId?: string;
  timeout?: number;
  signal?: AbortSignal;
}) {
  const env = environment ?? {};
  signal?.throwIfAborted();
  const cancellationToken = crypto.randomUUID();
  const effectiveTimeout = Math.min(timeout ?? 60_000, 60_000);
  if (!Number.isFinite(effectiveTimeout) || effectiveTimeout < 0)
    throw new Error("Invalid process timeout");
  return new Promise<Awaited<ReturnType<typeof sendRunProcessRequest>>>((resolve, reject) => {
    let settled = false;
    const cleanup = () => {
      clearTimeout(timer);
      signal?.removeEventListener("abort", cancel);
    };
    const cancel = () => {
      if (settled) return;
      settled = true;
      cleanup();
      void sendCancelProcessRequest({ cancellationToken }).catch(() => {});
      reject(signal?.reason ?? new DOMException("Process deadline exceeded", "TimeoutError"));
    };
    const timer = setTimeout(cancel, effectiveTimeout + 1000);
    signal?.addEventListener("abort", cancel, { once: true });
    sendRunProcessRequest({
      cancellationToken,
      executable,
      arguments: args,
      env: Object.keys(env).map((key) =>
        create(EnvironmentVariableSchema, { key, value: env[key] }),
      ),
      workingDirectory,
      terminalSessionId,
      timeout: create(DurationSchema, {
        nanos: Math.floor((effectiveTimeout % 1000) * 1_000_000),
        secs: BigInt(Math.floor(effectiveTimeout / 1000)),
      }),
    }).then(
      (response) => {
        if (!settled) {
          settled = true;
          cleanup();
          resolve(response);
        }
      },
      (error) => {
        if (!settled) {
          settled = true;
          cleanup();
          reject(error);
        }
      },
    );
  });
}
