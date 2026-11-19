import { Fig, Internal } from "@fig/api-bindings";
import { useState } from "react";
import { useRefreshLocalState } from "@/hooks/store/useState";
import { Button } from "../../ui/button";
import { TerminalSetup } from "../statusCheck";

export default function OnboardingModal() {
  const refresh = useRefreshLocalState();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  async function finish() {
    setBusy(true);
    try {
      await Internal.sendOnboardingRequest({ action: Fig.OnboardingAction.FINISH_ONBOARDING });
      await refresh();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }
  return (
    <div className="max-h-[75vh] overflow-y-auto px-1">
      <TerminalSetup />
      <p className="text-sm mb-3">You can return to these controls in Settings → Integrations.</p>
      {error && <p role="alert">{error}</p>}
      <Button disabled={busy} onClick={() => void finish()}>
        Continue to settings
      </Button>
    </div>
  );
}
