/**
 * The order the arms are sampled in, one permutation per repetition. Balanced,
 * because a fixed order and independent shuffles both leave an arm favouring
 * one end of the sweep; seeded, because `Math.random` would not reproduce.
 */

/** mulberry32: small, well-distributed, and deterministic from `seed`. */
const rng = (seed: number) => () => {
  seed = (seed + 0x6d2b79f5) | 0;
  let t = Math.imul(seed ^ (seed >>> 15), 1 | seed);
  t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
  return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
};

const shuffle = <T>(items: readonly T[], seed: number): T[] => {
  const next = rng(seed);
  const out = [...items];
  for (let i = out.length - 1; i > 0; i--) {
    const j = Math.floor(next() * (i + 1));
    [out[i], out[j]] = [out[j]!, out[i]!];
  }
  return out;
};

/**
 * A Latin square, one row per repetition: over a complete cycle of
 * `items.length` repetitions every arm occupies every slot exactly once, and
 * every arm's mean sweep position is identical, so drift within a sweep cannot
 * be charged to arm identity. The base order is reshuffled each cycle.
 *
 * The leftover rows of an incomplete cycle take their rotations spread around
 * the cycle rather than consecutively — consecutive offsets give each arm a
 * contiguous block of slots, which is what pulled the 15-of-18 sweep's mean
 * positions to 7.00–10.00. Spread, they are 8.00–9.00 against an ideal 8.50.
 */
export const shuffled = <T>(items: readonly T[], rep: number, reps: number): T[] => {
  const n = items.length;
  const base = shuffle(items, Math.floor(rep / n) * 2654435761 + 1);
  const whole = Math.floor(reps / n) * n;
  const leftover = reps - whole;
  const offset =
    rep >= whole && leftover > 1 ? Math.round(((rep - whole) * n) / leftover) % n : rep % n;
  return base.map((_, i) => base[(i + offset) % n]!);
};
