/**
 * The order the arms are sampled in, one permutation per repetition.
 *
 * Rotating by one only balances positions when the repetition count is a
 * multiple of the arm count, and neither default is: 15 repetitions over 18
 * runs leaves every arm three slots it never occupies. A seeded shuffle has no
 * such structure, and seeding it keeps the run reproducible — `Math.random`
 * would not.
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
