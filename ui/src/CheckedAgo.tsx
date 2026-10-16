// "checked N s ago": how fresh Forge data (pull requests, GitHub Issues) is. The Forge is
// the one polled source, so only its views show this; local data is pushed and carries none.
import { useEffect, useState } from "react";

/** "checked 12 s ago", "checked 3 min ago". */
export function checkedAgo(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000));
  return s < 60 ? `checked ${s} s ago` : `checked ${Math.floor(s / 60)} min ago`;
}

/** The time since `at` (ms since the epoch). The timer only repaints the label; it reads no
 * data, and it stops while the window is hidden so it never wakes a background window. */
export function CheckedAgo({ at }: { at: number }) {
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    let timer: ReturnType<typeof setInterval> | undefined;
    const follow = () => {
      clearInterval(timer);
      timer = undefined;
      if (document.visibilityState === "hidden") return;
      setNow(Date.now());
      timer = setInterval(() => setNow(Date.now()), 1000);
    };
    follow();
    document.addEventListener("visibilitychange", follow);
    return () => {
      clearInterval(timer);
      document.removeEventListener("visibilitychange", follow);
    };
  }, []);
  return (
    <span className="checked-ago" title={new Date(at).toLocaleString()}>
      {checkedAgo(Math.max(now, Date.now()) - at)}
    </span>
  );
}
