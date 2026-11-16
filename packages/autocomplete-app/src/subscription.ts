import logger from "loglevel";

type Subscription = { unsubscribe(): void };

/** Retry transient registration failures without needing to remount the app or restart Fig. */
export function subscribeWithRetry(getSubscription: () => Promise<Subscription> | undefined) {
  let stopped = false;
  let subscription: Subscription | undefined;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let delay = 1000;

  const connect = async () => {
    try {
      const result = await getSubscription();
      if (stopped) result?.unsubscribe();
      else subscription = result;
    } catch (error) {
      if (stopped) return;
      logger.warn("Fig subscription failed; retrying", error);
      timer = setTimeout(() => void connect(), delay);
      delay = Math.min(delay * 2, 30_000);
    }
  };
  void connect();

  return () => {
    stopped = true;
    clearTimeout(timer);
    subscription?.unsubscribe();
  };
}
