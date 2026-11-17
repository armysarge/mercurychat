// Time until a UTC instant, as the strip and the beacon card show it.

/** "00:14:42" until `utc` (never negative: a past time is "00:00:00"). */
export function countdown(utc: string, now: number): string {
  const s = Math.max(0, Math.ceil((Date.parse(utc) - now) / 1000));
  const two = (n: number) => String(n).padStart(2, "0");
  return `${two(Math.floor(s / 3600))}:${two(Math.floor((s % 3600) / 60))}:${two(s % 60)}`;
}
