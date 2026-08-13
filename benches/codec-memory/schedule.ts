/**
 * The order the arms are sampled in, one permutation per repetition. Rotating
 * balances positions only when the repetitions are a multiple of the arms, and
 * neither default is; seeded, because `Math.random` would not reproduce.
 */

/** mulberry32: small, well-distributed, and deterministic from `seed`. */
const rng = (seed: number) => () => {
  seed = (seed + 0x6d2b79f5) | 0;
  let t = Math.imul(seed ^ (seed >>> 15), 1 | seed);
  t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
  return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
};

export const shuffled = <T>(items: readonly T[], rep: number): T[] => {
  const next = rng(rep * 2654435761);
  const out = [...items];
  for (let i = out.length - 1; i > 0; i--) {
    const j = Math.floor(next() * (i + 1));
    [out[i], out[j]] = [out[j]!, out[i]!];
  }
  return out;
};
