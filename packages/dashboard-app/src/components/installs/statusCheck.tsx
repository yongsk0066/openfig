import { useEffect } from "react";
import { checks } from "@/data/install";
import { useSetup } from "@/hooks/store/useStatusCheck";
import { useStoreContext } from "@/hooks/store/useStoreContext";
import type { SetupKey } from "@/lib/store";
import { Button } from "../ui/button";

export function SetupRow({ id }: { id: SetupKey }) {
  const checkSetup = useStoreContext((s) => s.checkSetup);
  const configure = useStoreContext((s) => s.configure);
  const current = useSetup(id);
  const check = checks[id];
  useEffect(() => {
    const refresh = () => {
      void checkSetup(id);
    };
    refresh();
    window.addEventListener("focus", refresh);
    return () => window.removeEventListener("focus", refresh);
  }, [id, checkSetup]);
  const status = current?.status;
  const action =
    id === "ghosttyInputMethod" && status === "ready"
      ? "Reopen input method"
      : id === "ghosttyInputMethod" && status === "selection"
        ? "Finish activation"
        : check.action;
  return (
    <section className="border-b border-zinc-200 dark:border-zinc-600 py-3 space-y-2">
      <h3 className="font-medium">{check.title}</h3>
      <p className="text-sm text-zinc-600 dark:text-zinc-300">{check.detail}</p>
      <output className="block text-sm font-medium">
        {current?.busy
          ? "Applying…"
          : status
            ? (check.status[status] ?? "Unavailable")
            : current?.error
              ? "Check failed"
              : "Checking…"}
      </output>
      {current?.error && (
        <p role="alert" className="text-sm break-words text-red-700 dark:text-red-300">
          {current.error}
        </p>
      )}
      <div className="flex flex-wrap gap-2">
        {(status !== "ready" || check.alwaysAction) && (
          <Button
            size="sm"
            disabled={current?.busy || status === "unsupported"}
            onClick={() => void configure(id)}
          >
            {action}
          </Button>
        )}
        <Button
          size="sm"
          variant="outline"
          disabled={current?.busy}
          onClick={() => void checkSetup(id)}
        >
          Check again
        </Button>
      </div>
    </section>
  );
}

export function TerminalSetup() {
  return (
    <section aria-label="Terminal setup" className="mb-4">
      <h2 className="font-bold text-lg">Terminal setup</h2>
      <SetupRow id="dotfiles" />
      <SetupRow id="accessibility" />
      <SetupRow id="ghosttyInputMethod" />
    </section>
  );
}
