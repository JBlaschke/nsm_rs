//! Deterministic pseudo-random values for the randomized ("property style")
//! unit tests.
//!
//! A fixed seed makes every run reproducible without pulling a property
//! testing framework into the dependency tree; when a case fails, the seed
//! and the iteration number in the assertion message identify it.

use crate::net::{Addr, Transport};
use crate::protocol::RegToken;

/// xorshift64*: small, fast, and good enough to shake out parser and codec
/// corner cases.
pub(crate) struct Rng(u64);

impl Rng {
    pub(crate) fn new(seed: u64) -> Self {
        Rng(seed.max(1))
    }

    pub(crate) fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// A value in `0..n`.
    pub(crate) fn below(&mut self, n: usize) -> usize {
        assert!(n > 0, "below(0)");
        (self.next_u64() % n as u64) as usize
    }

    /// A value in `lo..=hi`.
    pub(crate) fn range(&mut self, lo: usize, hi: usize) -> usize {
        lo + self.below(hi - lo + 1)
    }

    /// True once in `one_in` draws on average.
    pub(crate) fn chance(&mut self, one_in: usize) -> bool {
        self.below(one_in) == 0
    }

    pub(crate) fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }

    /// Up to `max_len` characters drawn from `alphabet`.
    pub(crate) fn string(&mut self, alphabet: &[char], max_len: usize) -> String {
        let len = self.below(max_len + 1);
        (0..len).map(|_| *self.pick(alphabet)).collect()
    }

    /// Text with everything JSON must escape plus a few non-ASCII characters.
    pub(crate) fn text(&mut self, max_len: usize) -> String {
        const ALPHABET: &[char] = &[
            'a', 'b', 'z', 'A', '0', '9', ' ', '"', '\\', '/', '\n', '\t', '\r', '\0', 'é', 'λ',
            '😀', '\u{2028}', '<', '>', '&', '{', '}', ':', ',', '[', ']',
        ];
        self.string(ALPHABET, max_len)
    }

    pub(crate) fn bytes(&mut self, max_len: usize) -> Vec<u8> {
        let len = self.below(max_len + 1);
        (0..len).map(|_| self.next_u64() as u8).collect()
    }

    /// An IPv4 literal, an IPv6 literal or a lowercase DNS name.
    pub(crate) fn host(&mut self) -> String {
        match self.below(3) {
            0 => std::net::Ipv4Addr::from(self.next_u64() as u32).to_string(),
            1 => {
                let bits = (u128::from(self.next_u64()) << 64) | u128::from(self.next_u64());
                std::net::Ipv6Addr::from(bits).to_string()
            }
            _ => {
                const LABEL: &[char] = &['a', 'b', 'c', 'x', 'y', 'z', '0', '1', '9', '-'];
                let labels = self.range(1, 4);
                (0..labels)
                    .map(|_| {
                        let label = self.string(LABEL, 8);
                        if label.is_empty() || label.starts_with('-') || label.ends_with('-') {
                            format!("h{label}h")
                        } else {
                            label
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(".")
            }
        }
    }

    pub(crate) fn transport(&mut self) -> Transport {
        *self.pick(&[
            Transport::Tcp,
            Transport::Tls,
            Transport::Http,
            Transport::Https,
        ])
    }

    pub(crate) fn addr(&mut self) -> Addr {
        let transport = self.transport();
        let host = self.host();
        Addr::new(transport, host, self.next_u64() as u16)
    }

    pub(crate) fn token(&mut self) -> RegToken {
        let mut bytes = [0u8; 16];
        bytes[..8].copy_from_slice(&self.next_u64().to_le_bytes());
        bytes[8..].copy_from_slice(&self.next_u64().to_le_bytes());
        RegToken::from_bytes(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::Rng;

    #[test]
    fn same_seed_same_sequence_and_values_spread() {
        let mut a = Rng::new(1);
        let mut b = Rng::new(1);
        let seq: Vec<u64> = (0..8).map(|_| a.next_u64()).collect();
        assert_eq!(seq, (0..8).map(|_| b.next_u64()).collect::<Vec<_>>());
        let mut seen = std::collections::HashSet::new();
        for _ in 0..1000 {
            seen.insert(a.below(10));
        }
        assert_eq!(seen.len(), 10);
        assert!((0..100).all(|_| (3..=5).contains(&a.range(3, 5))));
        assert!(a.string(&['x'], 0).is_empty());
    }

    #[test]
    fn hosts_are_valid_authorities() {
        let mut rng = Rng::new(3);
        for _ in 0..500 {
            let host = rng.host();
            assert!(!host.is_empty());
            assert!(!host.contains(['/', '@', '?', '#', ' ']));
            if host.parse::<std::net::IpAddr>().is_err() {
                assert!(host
                    .split('.')
                    .all(|l| !l.is_empty() && !l.starts_with('-') && !l.ends_with('-')));
            }
        }
    }
}
