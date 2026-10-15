// A virtualised vertical list for rows of varying height: only the rows in (and just around)
// the viewport are in the DOM. Heights start as estimates and are corrected as rows are
// measured. While the user is at the bottom, new rows keep it pinned there (streaming).
import { useCallback, useLayoutEffect, useMemo, useReducer, useRef, useState, type ReactNode } from "react";

const OVERSCAN_PX = 600;
/** Used when the container has no size yet (first paint, tests without layout). */
const FALLBACK_VIEWPORT = 800;
const PIN_SLACK_PX = 40;

export interface VirtualListProps {
  count: number;
  /** Changes whenever any row's content may have changed, so pinned lists follow it. */
  version?: unknown;
  estimate: (index: number) => number;
  renderRow: (index: number) => ReactNode;
  label: string;
  className?: string;
  /** Scroll row `index` into view; a new `seq` asks again. */
  reveal?: { index: number; seq: number };
}

export function VirtualList({ count, version, estimate, renderRow, label, className, reveal }: VirtualListProps) {
  const scroller = useRef<HTMLDivElement>(null);
  const measured = useRef(new Map<number, number>());
  const [measures, remeasured] = useReducer((n: number) => n + 1, 0);
  const [scrollTop, setScrollTop] = useState(0);
  const pinned = useRef(true);

  const offsets = useMemo(() => {
    const tops = new Float64Array(count + 1);
    for (let i = 0; i < count; i++) tops[i + 1] = tops[i] + (measured.current.get(i) ?? estimate(i));
    return tops;
    // `measures` counts measurements: the map itself is read live.
  }, [count, estimate, version, measures]);

  const viewport = scroller.current?.clientHeight || FALLBACK_VIEWPORT;
  const first = Math.max(0, findRow(offsets, scrollTop - OVERSCAN_PX));
  const last = Math.min(count - 1, findRow(offsets, scrollTop + viewport + OVERSCAN_PX));

  useLayoutEffect(() => {
    const el = scroller.current;
    if (el && pinned.current) el.scrollTop = el.scrollHeight;
  }, [count, version, offsets]);

  const revealIndex = reveal?.index;
  const revealSeq = reveal?.seq;
  useLayoutEffect(() => {
    if (revealIndex === undefined || revealIndex >= count) return;
    const top = Math.max(0, offsets[revealIndex] - viewport / 3);
    pinned.current = false;
    if (scroller.current) scroller.current.scrollTop = top;
    setScrollTop(top);
    // Only a new request to reveal scrolls; streaming rows afterwards do not.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [revealIndex, revealSeq]);

  const observer = useMemo(
    () =>
      typeof ResizeObserver === "undefined"
        ? null
        : new ResizeObserver((items) => {
            let changed = false;
            for (const item of items) {
              const index = Number((item.target as HTMLElement).dataset.index);
              const height = (item.target as HTMLElement).offsetHeight;
              if (height > 0 && measured.current.get(index) !== height) {
                measured.current.set(index, height);
                changed = true;
              }
            }
            if (changed) remeasured();
          }),
    [],
  );
  useLayoutEffect(() => () => observer?.disconnect(), [observer]);

  const rowRef = useCallback(
    (el: HTMLDivElement | null) => {
      if (!el) return;
      observer?.observe(el);
    },
    [observer],
  );

  const rows: ReactNode[] = [];
  for (let i = first; i <= last; i++)
    rows.push(
      <div key={i} data-index={i} ref={rowRef} className="vrow" style={{ top: offsets[i] }}>
        {renderRow(i)}
      </div>,
    );

  return (
    <div
      ref={scroller}
      className={className}
      role="log"
      aria-label={label}
      onScroll={(e) => {
        const el = e.currentTarget;
        pinned.current = el.scrollHeight - el.scrollTop - el.clientHeight < PIN_SLACK_PX;
        setScrollTop(el.scrollTop);
      }}
    >
      <div className="vspace" style={{ height: offsets[count] }}>
        {rows}
      </div>
    </div>
  );
}

/** The row containing pixel offset `y` (binary search over row tops). */
function findRow(tops: Float64Array, y: number): number {
  let lo = 0;
  let hi = tops.length - 2;
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if (tops[mid] <= y) lo = mid;
    else hi = mid - 1;
  }
  return lo;
}
