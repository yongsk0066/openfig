import { useImperativeHandle, type Ref } from "react";
import { List, type ListProps, useListRef } from "react-window";
import { twMerge } from "tailwind-merge";
import { useDynamicResizeObserver } from "../hooks/helpers";

type ResizeHandler = (size: { width?: number; height?: number }) => void;

type AutoSizedListProps<RowProps extends object> = Omit<
  ListProps<RowProps>,
  "style" | "listRef" | "onResize" | "rowHeight"
> & {
  ref?: Ref<AutoSizedHandleRef>;
  rowHeight: number;
  width: number | string | undefined;
  onResize: ResizeHandler | undefined;
};

export type AutoSizedHandleRef = {
  scrollToItem: (index: number) => void;
};

// List will attempt to be size (rowCount * rowHeight) but will shrink and
// scroll if necessary.
export default function AutoSizedList<RowProps extends object>({
  ref,
  onResize = undefined,
  width: desiredWidth = undefined,
  className,
  rowCount,
  rowHeight,
  ...props
}: AutoSizedListProps<RowProps>) {
  const { ref: wrapperRef, height, width } = useDynamicResizeObserver({ onResize });
  const listRef = useListRef(null);

  useImperativeHandle(
    ref,
    () => ({
      scrollToItem: (index) => {
        if (index >= 0 && index < rowCount) {
          listRef.current?.scrollToRow({ index, align: "smart" });
        }
      },
    }),
    [listRef, rowCount],
  );

  return (
    <div
      ref={wrapperRef}
      style={{
        flexBasis: rowCount * rowHeight,
      }}
      className="min-h-0 min-w-0 flex-shrink"
    >
      <List
        listRef={listRef}
        rowCount={rowCount}
        rowHeight={rowHeight}
        className={twMerge("scrollbar-none", className)}
        style={{
          height: height || 0,
          width: desiredWidth === undefined ? width || 0 : desiredWidth,
        }}
        {...props}
      />
    </div>
  );
}
