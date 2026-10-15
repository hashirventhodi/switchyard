// An immutable list that appends and replaces in O(chunk) instead of O(n): a transcript of tens
// of thousands of entries grows one event at a time without copying itself each time.

const CHUNK = 256;

export interface Chunked<T> {
  readonly chunks: readonly (readonly T[])[];
  readonly length: number;
}

export const emptyChunked = <T>(): Chunked<T> => ({ chunks: [], length: 0 });

export function at<T>(list: Chunked<T>, index: number): T | undefined {
  return list.chunks[Math.floor(index / CHUNK)]?.[index % CHUNK];
}

/** Appends when `index === length`, replaces when lower; ignores gaps. */
export function put<T>(list: Chunked<T>, index: number, value: T): Chunked<T> {
  if (index > list.length || index < 0) return list;
  const c = Math.floor(index / CHUNK);
  const chunks = list.chunks.slice();
  const chunk = (chunks[c] ?? []).slice();
  chunk[index % CHUNK] = value;
  chunks[c] = chunk;
  return { chunks, length: index === list.length ? list.length + 1 : list.length };
}
