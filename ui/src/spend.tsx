// Spend: a session's cost and its breakdown by Model (header) and Spend today (status bar,
// Dashboard KPI). The core sends running totals (`spendRecorded`); a cost is either
// reported by the CLI or estimated from the price table, and the two sums are never folded
// into one figure without a tilde marking the estimate. A Model with neither is unpriced:
// shown as such with its tokens, never as $0.
import { useEffect, useState } from "react";
import type { ModelSpend } from "./schema/ModelSpend";
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

const tokensText = (s: Spend | ModelSpend) =>
  `${count(s.inputTokens)} in · ${count(s.outputTokens)} out · ${count(s.cacheReadTokens)} cache reads · ${count(s.cacheWriteTokens ?? 0)} cache writes`;

/** How a Model is named; null when the agent named none and the Session's is not known. */
const modelName = (m: ModelSpend) => m.model ?? "the Session's Model";

const isUnpriced = (m: ModelSpend) => m.reportedUsd === null && m.estimatedUsd === null;

/** `s`'s cost: [value, tooltip parts], or null when none of it is priced. */
function priced(s: Spend | ModelSpend): [string, string[]] | null {
  const { reportedUsd: reported, estimatedUsd: estimated } = s;
  if (reported === null && estimated === null) return null;
  const parts = [
    ...(reported !== null ? [`Reported by the CLI: $${reported}`] : []),
    ...(estimated !== null ? [`Estimated from price table ${s.priceTableVersion}: ~$${estimated.toFixed(4)}`] : []),
  ];
  return [totalsText({ reported: reported ?? 0, estimated: estimated ?? 0 }), parts];
}

/** A session's cost for the summary strip: [value, tooltip]. */
export function sessionCost(session: SessionView): [string, string] {
  const latest = session.spend[session.spend.length - 1]?.spend;
  if (!latest) return ["—", "No usage reported yet"];
  const unpriced = latest.byModel.filter(isUnpriced).map(modelName);
  const cost = priced(latest);
  if (!cost) {
    const which = unpriced.length ? unpriced.join(", ") : "this Model";
    return ["Unpriced", `No price for ${which} in the price table · ${tokensText(latest)}`];
  }
  const [value, parts] = cost;
  if (!unpriced.length) return [value, [...parts, tokensText(latest)].join(" · ")];
  return [`${value} + unpriced`, [...parts, `Unpriced: ${unpriced.join(", ")}`, tokensText(latest)].join(" · ")];
}

/** What each Model cost, when a Session used more than one. */
export function SpendByModel({ session }: { session: SessionView }) {
  const models = session.spend[session.spend.length - 1]?.spend.byModel ?? [];
  if (models.length < 2) return null;
  return (
    <div className="c by-model">
      <div className="l">By Model</div>
      <ul aria-label="Spend by Model">
        {models.map((m) => {
          const cost = priced(m);
          const [value, parts] = cost ?? ["unpriced", [`No price for ${modelName(m)} in the price table`]];
          return (
            <li key={modelName(m)} title={[...parts, tokensText(m)].join(" · ")}>
              <span className="m">{modelName(m)}</span> <span className={cost ? "v" : "v unpriced"}>{value}</span>
            </li>
          );
        })}
      </ul>
    </div>
  );
}
