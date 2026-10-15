import { expect, test } from "vitest";
import { emptyRawLog, pushRaw, rawAt, RAW_LOG_BYTES } from "../rawLog";

test("keeps every event while small, numbered from the first one received", () => {
  let log = emptyRawLog();
  log = pushRaw(log, 500, "a");
  log = pushRaw(log, 501, "b");
  expect(log.first).toBe(500);
  expect(log.length).toBe(2);
  expect(rawAt(log, 0)).toBe("a");
  expect(rawAt(log, 1)).toBe("b");
});

test("drops the oldest once past the byte budget, as the core does", () => {
  let log = emptyRawLog();
  const big = "x".repeat(10 * 1024);
  for (let i = 0; i < 100; i++) log = pushRaw(log, i, big);
  log = pushRaw(log, 100, "last");
  expect(log.bytes).toBeLessThanOrEqual(RAW_LOG_BYTES);
  expect(log.length).toBeGreaterThan(10);
  expect(log.first + log.length).toBe(101);
  expect(rawAt(log, log.length - 1)).toBe("last");
});

test("a gap (the core dropped some) starts again from the new number; repeats are ignored", () => {
  let log = pushRaw(pushRaw(emptyRawLog(), 0, "a"), 1, "b");
  expect(pushRaw(log, 1, "b again")).toBe(log);
  log = pushRaw(log, 40, "z");
  expect(log.first).toBe(40);
  expect(log.length).toBe(1);
  expect(rawAt(log, 0)).toBe("z");
});
