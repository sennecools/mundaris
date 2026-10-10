//! Counter-based random streams with domain separation.
//!
//! Every random draw in the grower is `lattice_hash(key, domain, a, b)` from
//! `astrum_math::noise` (the M0 SplitMix64 lattice hash), so there is one hash
//! in the engine and no stateful RNG. A draw depends only on its seed, its
//! domain tag and its counters, never on how many draws came before it in some
//! other domain. Changing how many markers are sampled therefore never shifts
//! the bud angles, and so on.

use astrum_math::noise::lattice_hash;

/// Domain tags. Each is a distinct constant; never reuse one for two purposes.
pub mod domain {
    pub const MARKERS: u64 = 0x464c_0001;
    pub const BUD_ROLL: u64 = 0x464c_0002;
    pub const BUD_JITTER: u64 = 0x464c_0003;
    pub const ORGAN: u64 = 0x464c_0004;
    pub const STEM: u64 = 0x464c_0005;
    pub const TUFT: u64 = 0x464c_0006;
    pub const VARIANT: u64 = 0x464c_0007;
    pub const FRUIT: u64 = 0x464c_0008;
}

/// A keyed stream: `key` is the species/variant seed, `domain` the purpose.
#[derive(Debug, Clone, Copy)]
pub struct Stream {
    key: u64,
    domain: u64,
}

impl Stream {
    pub const fn new(key: u64, domain: u64) -> Self {
        Self { key, domain }
    }

    /// 64 random bits for counter `(a, b)`.
    pub fn bits(&self, a: u64, b: u64) -> u64 {
        lattice_hash(self.key, self.domain as i64, a as i64, b as i64)
    }

    /// Uniform in `[0, 1)` from the top 53 bits.
    pub fn unit(&self, a: u64, b: u64) -> f64 {
        (self.bits(a, b) >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }

    /// Uniform in `[-1, 1)`.
    pub fn signed(&self, a: u64, b: u64) -> f64 {
        self.unit(a, b) * 2.0 - 1.0
    }

    /// Uniform in `[lo, hi)`.
    pub fn range(&self, a: u64, b: u64, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.unit(a, b)
    }
}

/// Derive a sub-key (e.g. variant seed from species seed).
pub fn derive(key: u64, tag: u64) -> u64 {
    lattice_hash(key, domain::VARIANT as i64, tag as i64, 0)
}

/// Stable 64-bit hash of a name (FNV-1a), used to seed species from their file name.
pub fn name_key(name: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in name.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domains_are_independent() {
        let a = Stream::new(7, domain::MARKERS);
        let b = Stream::new(7, domain::BUD_ROLL);
        assert_ne!(a.bits(1, 2), b.bits(1, 2));
        assert_eq!(a.bits(1, 2), Stream::new(7, domain::MARKERS).bits(1, 2));
    }

    #[test]
    fn unit_in_range() {
        let s = Stream::new(1, domain::ORGAN);
        for i in 0..1000 {
            let u = s.unit(i, 0);
            assert!((0.0..1.0).contains(&u));
        }
    }
}
