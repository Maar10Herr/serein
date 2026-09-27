/** Foreground time is a bounded estimate. Unknown intervals never exceed 65s. */
export function foregroundDelta(
  lastTick: number,
  now: number,
  active: boolean,
) {
  return active ? Math.min(65, Math.max(0, (now - lastTick) / 1000)) : 0;
}
export function shouldRetain(kind: "visit" | "search", seconds: number) {
  return kind === "search" || seconds >= 5;
}
