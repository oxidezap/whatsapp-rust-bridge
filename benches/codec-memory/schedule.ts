/**
 * The order the arms are sampled in, one permutation per repetition. Balanced,
 * because independent shuffles still leave an arm favouring a slot over five or
 * fifteen repetitions; seeded, because `Math.random` would not reproduce.
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
 * A Latin square, one row per repetition: within any `items.length` consecutive
 * repetitions every arm occupies every slot exactly once, so a partial cycle is
 * off by at most one. The base order is reshuffled each cycle, so no pair of
 * arms stays adjacent across the run.
 */
export const shuffled = <T>(items: readonly T[], rep: number): T[] => {
  const n = items.length;
  const base = shuffle(items, Math.floor(rep / n) * 2654435761 + 1);
  const offset = rep % n;
  return base.map((_, i) => base[(i + offset) % n]!);
};
