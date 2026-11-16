import { useState, useEffect, useMemo, useCallback, type RefCallback } from "react";
const getSystemTheme = () =>
  window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";

type Dimensions = { height?: number; width?: number };

export const useDynamicResizeObserver = <T extends Element = HTMLDivElement>(opts?: {
  onResize?: (size: Dimensions) => void;
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
  useEffect(() => {
    onResize?.(dimensions);
  }, [dimensions, onResize]);
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
