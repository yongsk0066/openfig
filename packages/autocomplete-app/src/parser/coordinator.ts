import type { createParser } from "@fig/autocomplete-parser/core";
type Parse = ReturnType<typeof createParser>["parseArguments"];

type Command = Parameters<Parse>[0];
type Context = Parameters<Parse>[1];
type Result = Awaited<ReturnType<Parse>>;

export function bufferChange(previous: Command, command: Command) {
  const tokens = command?.tokens ?? [];
  const oldTokens = previous?.tokens ?? [];
  const text = command?.originalTree.text ?? "";
  const oldText = previous?.originalTree.text ?? "";
  return {
    backspaced:
      tokens.length > 0 &&
      tokens.length < oldTokens.length &&
      oldTokens[tokens.length - 1]?.text === tokens[tokens.length - 1]?.text,
    large:
      (!text.startsWith(oldText) && !oldText.startsWith(text)) ||
      Math.abs(text.length - oldText.length) >= 2,
  };
}

/** Owns result delivery independently of React and the WebView bridge. */
export function createParseCoordinator(
  parse: (command: Command, context: Context, signal?: AbortSignal) => Promise<Result>,
) {
  let generation = 0;
  let previous: Command = null;
  return {
    run(
      command: Command,
      context: Context,
      callbacks: {
        loading: (value: true) => void;
        result: (result: Result, backspaced: boolean, large: boolean) => void;
        error: (error: unknown) => void;
      },
    ) {
      const current = ++generation;
      const controller = new AbortController();
      const change = bufferChange(previous, command);
      previous = command;
      callbacks.loading(true);
      void parse(command, context, controller.signal).then(
        (result) => {
          if (current !== generation) return;
          callbacks.result(result, change.backspaced, change.large);
        },
        (error) => {
          if (current !== generation) return;
          callbacks.error(error);
        },
      );
      return () => {
        controller.abort();
        if (current === generation) generation++;
      };
    },
  };
}
