import {
  useState,
  useEffect,
  useLayoutEffect,
  useRef,
  useMemo,
  useCallback,
  type RefCallback,
} from "react";
const getSystemTheme = () =>
  window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";

type Dimensions = { height?: number; width?: number };

export const useDynamicResizeObserver = <T extends Element = HTMLDivElement>(opts?: {
  onResize?: (size: Dimensions) => void;
  /** Notify from committed layout; returned dimensions remain the observer snapshot. */
  measureOnCommit?: boolean;
}): Dimensions & { ref: RefCallback<T> } => {
  const [element, setElement] = useState<T | null>(null);
  const [dimensions, setDimensions] = useState<Dimensions>({ height: 0, width: 0 });
  const ref = useCallback<RefCallback<T>>((node) => setElement(node), []);
  useEffect(() => {
    if (!element) return;
    let active = true;
    const observer = new ResizeObserver(([entry]) => {
      if (!active || !entry) return;
      const width = Math.round(entry.contentRect.width);
      const height = Math.round(entry.contentRect.height);
      setDimensions((old) =>
        old.width === width && old.height === height ? old : { width, height },
      );
    });
    observer.observe(element);
    return () => {
      active = false;
      observer.disconnect();
    };
  }, [element]);
  const onResize = opts?.onResize;
  const measureOnCommit = opts?.measureOnCommit ?? false;
  const lastNotification = useRef<
    { size: Dimensions; callback: typeof onResize; element: T } | undefined
  >(undefined);
  // A hidden WKWebView can defer ResizeObserver/RAF indefinitely. Native window
  // visibility must use this commit's layout, never replay a previous zero size.
  useLayoutEffect(() => {
    if (!element) return;
    const size = measureOnCommit
      ? (() => {
          const rect = element.getBoundingClientRect();
          return { width: Math.round(rect.width), height: Math.round(rect.height) };
        })()
      : dimensions;
    const previous = lastNotification.current;
    if (
      previous?.element !== element ||
      previous.callback !== onResize ||
      previous.size.width !== size.width ||
      previous.size.height !== size.height
    ) {
      lastNotification.current = { element, callback: onResize, size };
      onResize?.(size);
    }
  });
  return useMemo(() => ({ ...dimensions, ref }), [dimensions, ref]);
};

export const useShake = (): [boolean, () => void] => {
  const [isShaking, setShaking] = useState(false);
  const shake = useCallback((ms = 200) => {
    setShaking(true);
    setTimeout(() => {
      setShaking(false);
    }, ms);
  }, []);
  return [isShaking, shake];
};

export type SystemTheme = "light" | "dark";

export const useSystemTheme = (): [SystemTheme] => {
  const [systemTheme, setSystemTheme] = useState<SystemTheme>(getSystemTheme());
  useEffect(() => {
    const listener = (e: MediaQueryListEvent) => setSystemTheme(e.matches ? "dark" : "light");
    const query = window.matchMedia("(prefers-color-scheme: dark)");
    query.addEventListener("change", listener);
    return () => query.removeEventListener("change", listener);
  }, []);
  return [systemTheme];
};
