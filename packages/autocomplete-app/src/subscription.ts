import logger from "loglevel";
import type { Timing } from "@fig/shared/timing";
type Subscription = { unsubscribe(): void };

/** Bounded backoff; cancellation owns both the pending wait and a late registration. */
export function subscribeWithRetry(
  getSubscription: () => Promise<Subscription> | undefined,
  delay: Timing["delay"],
) {
  const scope = new AbortController();
  let subscription: Subscription | undefined;
  void (async () => {
    let backoff = 1000;
    while (!scope.signal.aborted) {
      try {
        const result = await getSubscription();
        if (scope.signal.aborted) result?.unsubscribe();
        else subscription = result;
        return;
      } catch (error) {
        if (scope.signal.aborted) return;
        logger.warn("Fig subscription failed; retrying", error);
        try {
          await delay(backoff, scope.signal);
        } catch {
          return;
        }
        backoff = Math.min(backoff * 2, 30_000);
      }
    }
  })();
  return () => {
    scope.abort();
    subscription?.unsubscribe();
  };
}
