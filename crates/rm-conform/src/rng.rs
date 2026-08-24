//! SplitMix64. Written here because the workspace takes no `rand` dependency
//! and this needs twenty lines, not a crate.
//!
//! The same reasoning as `rm_embed`: this project pays for its dependencies
//! deliberately.
//!
//! Determinism is the requirement rather than statistical quality. A failure
//! that cannot be reproduced from its seed is not a finding.

pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A value in `[0, n)`. Panics on `n == 0`, which is a caller bug.
    pub fn below(&mut self, n: u64) -> u64 {
        assert!(n > 0, "below(0) has no answer");
        self.next_u64() % n
    }

    /// True with probability `percent/100`.
    pub fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_seed_gives_the_same_sequence() {
        let a: Vec<u64> = {
            let mut r = Rng::new(42);
            (0..8).map(|_| r.next_u64()).collect()
        };
        let b: Vec<u64> = {
            let mut r = Rng::new(42);
            (0..8).map(|_| r.next_u64()).collect()
        };
        assert_eq!(a, b);
    }

    #[test]
    fn the_sequence_actually_advances() {
        // Without this, an Rng that returned a constant would satisfy the test
        // above and look perfectly reproducible.
        let mut r = Rng::new(42);
        let seq: Vec<u64> = (0..8).map(|_| r.next_u64()).collect();
        assert!(
            seq.windows(2).any(|w| w[0] != w[1]),
            "the generator returned a constant"
        );
    }

    #[test]
    fn different_seeds_diverge() {
        assert_ne!(Rng::new(1).next_u64(), Rng::new(2).next_u64());
    }

    #[test]
    fn below_stays_in_range() {
        let mut r = Rng::new(7);
        for _ in 0..1000 {
            assert!(r.below(5) < 5);
        }
    }

    #[test]
    fn below_reaches_every_value_in_range() {
        // `below` returning a constant would also pass the range test.
        let mut r = Rng::new(7);
        let mut seen = [false; 5];
        for _ in 0..1000 {
            seen[r.below(5) as usize] = true;
        }
        assert!(
            seen.iter().all(|s| *s),
            "some values never occurred: {seen:?}"
        );
    }
}
