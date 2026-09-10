import { useState, useEffect, useMemo, useCallback, type RefCallback, type RefObject } from "react";
import { useResizeObserver, type ResizeObserverBoxOptions } from "use-resize-observer";

type Dimensions = {
  height?: number;
  width?: number;
};

type DynamicResizeObserverOptions<T extends Element> = {
  ref?: RefObject<T | null> | T | null;
  onResize?: (size: Dimensions) => void;
  box?: ResizeObserverBoxOptions;
  round?: (n: number) => number;
};

type DynamicResizeObserverResponse<T extends Element> = Dimensions & {
  ref: RefCallback<T>;
};

// Like useResizeObserver but re-calls onResize callback when onResize changes.
export const useDynamicResizeObserver = <T extends Element = HTMLDivElement>(
  opts?: DynamicResizeObserverOptions<T>,
): DynamicResizeObserverResponse<T> => {
  const { onResize, ...otherOpts } = opts ?? {};
  const [{ height, width }, setDimensions] = useState<Dimensions>({
    height: 0,
    width: 0,
  });

  const { ref } = useResizeObserver<T>({
    ...otherOpts,
    onResize: ({ height: h, width: w }) => setDimensions({ height: h, width: w }),
  });

  useEffect(() => {
    if (onResize) {
      onResize({ height, width });
    }
  }, [height, width, onResize]);

  return useMemo(() => ({ height, width, ref }), [height, width, ref]);
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
  const getSystemTheme = () =>
    window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
  const [systemTheme, setSystemTheme] = useState<SystemTheme>(getSystemTheme());
  useEffect(() => {
    const listener = (e: MediaQueryListEvent) => setSystemTheme(e.matches ? "dark" : "light");
    const query = window.matchMedia("(prefers-color-scheme: dark)");
    query.addEventListener("change", listener);
    return () => query.removeEventListener("change", listener);
  }, []);
  return [systemTheme];
};
