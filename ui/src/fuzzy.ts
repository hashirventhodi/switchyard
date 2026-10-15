// Fuzzy matching for the command palette: the query's characters must appear in order
// (case-insensitive) in an item's title or one of its keywords (branch, task key, ...).

export interface Matchable {
  title: string;
  keywords?: string[];
}

const isWordChar = (c: string) => /[a-z0-9]/i.test(c);

/** Score of `query` against `text`, or null when it does not match. Higher is better. */
export function score(query: string, text: string): number | null {
  const q = query.toLowerCase();
  const t = text.toLowerCase();
  let best: number | null = null;
  // Try each place the first character occurs, so a later word-start alignment can win.
  for (let start = t.indexOf(q[0]); start !== -1; start = t.indexOf(q[0], start + 1)) {
    let total = 0;
    let prev = -2;
    let pos = start;
    let ok = true;
    for (const ch of q) {
      const at = t.indexOf(ch, pos);
      if (at === -1) {
        ok = false;
        break;
      }
      total += 1;
      if (at === prev + 1) total += 3;
      if (at === 0 || !isWordChar(t[at - 1])) total += 3;
      prev = at;
      pos = at + 1;
    }
    if (!ok) break;
    if (best === null || total > best) best = total;
  }
  return best;
}

/** Items matching `query`, best first; ties keep their original order. Empty query keeps all. */
export function rank<T extends Matchable>(query: string, items: T[]): T[] {
  const q = query.trim();
  if (!q) return items;
  return items
    .map((item, index) => {
      const scores = [item.title, ...(item.keywords ?? [])].map((t) => score(q, t)).filter((s) => s !== null);
      return { item, index, best: scores.length ? Math.max(...(scores as number[])) : null };
    })
    .filter((r) => r.best !== null)
    .sort((a, b) => b.best! - a.best! || a.index - b.index)
    .map((r) => r.item);
}
