// The newest raw adapter events of a session, for the Agent log's debug view. Bounded like
// the core's copy (256 KB of JSON): the oldest are dropped, the rest keep the numbers the
// core gave them. Stored in small immutable segments so an append copies at most one.

export const RAW_LOG_BYTES = 256 * 1024;
const SEGMENT = 64;

export interface RawLog {
  /** The core's number for the first kept event. */
  readonly first: number;
  readonly length: number;
  readonly bytes: number;
  readonly segments: readonly (readonly string[])[];
  /** Bytes in each segment. */
  readonly sizes: readonly number[];
}

export const emptyRawLog = (): RawLog => ({ first: 0, length: 0, bytes: 0, segments: [], sizes: [] });

/** The i-th kept event (0 = oldest kept). */
export function rawAt(log: RawLog, i: number): string | undefined {
  const head = log.segments[0]?.length ?? 0;
  if (i < head) return log.segments[0]?.[i];
  const rest = i - head;
  return log.segments[1 + Math.floor(rest / SEGMENT)]?.[rest % SEGMENT];
}

/** Add event `index` (the core's number). Appends the next number; a gap starts over. */
export function pushRaw(log: RawLog, index: number, json: string): RawLog {
  if (log.length > 0 && index < log.first + log.length) return log;
  if (log.length === 0 || index > log.first + log.length) {
    return { first: index, length: 1, bytes: json.length, segments: [[json]], sizes: [json.length] };
  }
  const segments = log.segments.slice();
  const sizes = log.sizes.slice();
  const last = segments.length - 1;
  if (segments[last].length < SEGMENT) {
    segments[last] = [...segments[last], json];
    sizes[last] += json.length;
  } else {
    segments.push([json]);
    sizes.push(json.length);
  }
  let { first } = log;
  let length = log.length + 1;
  let bytes = log.bytes + json.length;
  // Drop the oldest until within budget (always keeping the newest).
  while (bytes > RAW_LOG_BYTES && length > 1) {
    const head = segments[0];
    bytes -= head[0].length;
    sizes[0] -= head[0].length;
    first += 1;
    length -= 1;
    if (head.length === 1) {
      segments.shift();
      sizes.shift();
    } else {
      segments[0] = head.slice(1);
    }
  }
  return { first, length, bytes, segments, sizes };
}
