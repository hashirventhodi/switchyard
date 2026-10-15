// Spend: a session's cost (header) and Spend today (status bar, Dashboard KPI).
// The core sends running totals (`spendRecorded`); a cost is either reported by the CLI
// or estimated from the price table, and the two sums are never folded into one figure
// without a tilde marking the estimate.
import { useEffect, useState } from "react";
import type { Spend } from "./schema/Spend";
import type { SessionView } from "./state";

export interface SpendSample {
  atMs: number;
  spend: Spend;
}

export interface SpendTotals {
  reported: number;
  estimated: number;
}

const usd = (n: number) => `$${n.toFixed(2)}`;
const count = (n: number) => n.toLocaleString("en-US");

/** What `samples` (running totals, oldest first) add up to after `sinceMs`. */
function spentSince(samples: SpendSample[], sinceMs: number): SpendTotals {
  const latest = samples[samples.length - 1];
  if (!latest || latest.atMs < sinceMs) return { reported: 0, estimated: 0 };
  const before = samples.filter((s) => s.atMs < sinceMs).pop();
  return {
    reported: (latest.spend.reportedUsd ?? 0) - (before?.spend.reportedUsd ?? 0),
    estimated: (latest.spend.estimatedUsd ?? 0) - (before?.spend.estimatedUsd ?? 0),
  };
}

export function spendSinceAcross(sessions: SessionView[], sinceMs: number): SpendTotals {
  return sessions.reduce(
    (sum, s) => {
      const t = spentSince(s.spend, sinceMs);
      return { reported: sum.reported + t.reported, estimated: sum.estimated + t.estimated };
    },
    { reported: 0, estimated: 0 },
  );
}

function startOfLocalDay(ms: number): number {
  const d = new Date(ms);
  return new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
}

/** The start of today, local time; moves on by itself at midnight. */
export function useLocalMidnight(): number {
  const [midnight, setMidnight] = useState(() => startOfLocalDay(Date.now()));
  useEffect(() => {
    const d = new Date(midnight);
    const next = new Date(d.getFullYear(), d.getMonth(), d.getDate() + 1).getTime();
    const timer = setTimeout(() => setMidnight(startOfLocalDay(Date.now())), Math.max(0, next - Date.now()));
    return () => clearTimeout(timer);
  }, [midnight]);
  return midnight;
}

export function totalsText({ reported, estimated }: SpendTotals): string {
  return estimated > 0 ? `~${usd(reported + estimated)}` : usd(reported);
}

export function totalsTooltip({ reported, estimated }: SpendTotals): string {
  return `Reported ${usd(reported)} · Estimated ~${usd(estimated)}`;
}

/** Spend today across `sessions`, as the status bar or the Dashboard KPI shows it. */
export function SpendToday({ sessions, variant }: { sessions: SessionView[]; variant: "status" | "kpi" }) {
  const totals = spendSinceAcross(sessions, useLocalMidnight());
  if (variant === "status")
    return (
      <span className="i" data-testid="spend-today-status" title={totalsTooltip(totals)}>
        {totalsText(totals)} today
      </span>
    );
  return (
    <div className="kpi" data-testid="spend-today-kpi" title={totalsTooltip(totals)}>
      <div className="l">Spend today</div>
      <div className="v">{totalsText(totals)}</div>
    </div>
  );
}

/** A session's cost for the summary strip: [value, tooltip]. */
export function sessionCost(session: SessionView): [string, string] {
  const latest = session.spend[session.spend.length - 1]?.spend;
  if (!latest) return ["—", "No usage reported yet"];
  const tokens = `${count(latest.inputTokens)} in · ${count(latest.outputTokens)} out · ${count(latest.cacheReadTokens)} cached tokens`;
  if (latest.reportedUsd !== null) return [usd(latest.reportedUsd), `Reported by the CLI: $${latest.reportedUsd} · ${tokens}`];
  if (latest.estimatedUsd !== null)
    return [
      `~${usd(latest.estimatedUsd)}`,
      `Estimated from price table ${latest.priceTableVersion}: ~$${latest.estimatedUsd.toFixed(4)} · ${tokens}`,
    ];
  return ["—", `No price for this model in the price table · ${tokens}`];
}
