//! The store a client's claim shares with its service.
//!
//! One [`Store`] lives in each [`ClientEntry`](super::registry::ClientEntry):
//! the registry creates it empty when a claim is granted, keeps it across
//! re-pairings and drops it with the client (store plan, decision S2). Like
//! the registry it is pure: no I/O, no clock, and every method returns
//! immediately, so it runs inside the registry's critical section.
//!
//! # Versions
//!
//! A store does not number its own writes. Every applied write (a put, or a
//! delete that removed an entry) takes the next number from a source the
//! caller passes in, which the registry backs with one counter for the whole
//! broker (store plan, decision S5): a version then names one state of one
//! entry for the broker's whole life, and no two claims ever share a number.
//! An entry's `version` is the number of its last write, and the store's
//! `revision` is the last number it was issued, 0 before its first write.
//! Reads, refused operations and deletes of an absent key take no number.
//! When the source is exhausted the write is refused and nothing changes.
//!
//! # Budget
//!
//! Each entry costs [`entry_cost`]: its key and value as JSON-encoded strings
//! ([`json_len`]) plus [`ENTRY_OVERHEAD`]. A put is refused when the store's
//! total after the put would exceed the budget it is given; the budget is
//! checked before anything changes, so a refused put leaves the store as it
//! was. Counting encoded bytes rather than raw ones bounds the reply as well
//! as memory (escaping can make a value six times longer on the wire): a
//! `stored` reply carrying every entry of a full store is at most the budget
//! plus [`REPLY_OVERHEAD`] bytes, which is what
//! [`listen`](super::listen::listen) checks against the frame limit.
//!
//! Keys and values are never logged, and the `Debug` output of a [`Store`]
//! shows only its counts.

use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::fmt;

use crate::protocol::{StoreEntry, StoreKey, StoreOp};
use crate::{Error, Result};

/// Bytes counted for each entry on top of its encoded key and value: the
/// braces, the three field names, the separating comma and a 20-digit
/// version.
pub const ENTRY_OVERHEAD: usize = 64;

/// Bytes a `stored` reply needs on top of its entries: the `type` tag, a
/// 20-digit client id and revision, the field names and brackets, with room
/// to spare.
pub const REPLY_OVERHEAD: usize = 1024;

/// Length in bytes of `text` encoded as a JSON string by `serde_json`,
/// including both quotes.
///
/// A quote, a backslash and the control characters with a short escape
/// (`\b \t \n \f \r`) cost 2 bytes, every other byte below 0x20 costs 6
/// (`\u00XX`), and every other byte, non-ASCII included, costs 1.
/// Saturates instead of overflowing.
pub fn json_len(text: &str) -> usize {
    text.bytes().fold(2usize, |len, b| {
        let cost = match b {
            b'"' | b'\\' | 0x08 | b'\t' | b'\n' | 0x0c | b'\r' => 2,
            0x00..=0x1f => 6,
            _ => 1,
        };
        len.saturating_add(cost)
    })
}

/// Accounted size of one entry: its key and value as JSON-encoded strings
/// plus [`ENTRY_OVERHEAD`].
pub fn entry_cost(key: &StoreKey, value: &str) -> usize {
    json_len(key.as_str())
        .saturating_add(json_len(value))
        .saturating_add(ENTRY_OVERHEAD)
}

/// One stored value and the number of its last write.
#[derive(Clone, PartialEq, Eq)]
struct Slot {
    value: String,
    version: u64,
}

/// A small key-value map with a byte budget and write numbers from a
/// caller-supplied source. See the [module docs](self).
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Store {
    entries: BTreeMap<StoreKey, Slot>,
    revision: u64,
    bytes: usize,
}

impl fmt::Debug for Store {
    /// Counts only: never a key or a value.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Store")
            .field("entries", &self.entries.len())
            .field("bytes", &self.bytes)
            .field("revision", &self.revision)
            .finish()
    }
}

impl Store {
    /// Apply one operation and return the store's revision after it with
    /// the entries the operation returns: for a get, the entry or nothing;
    /// for a put, the entry as written; for a delete, the removed entry or
    /// nothing; for a list, every entry in ascending key order.
    ///
    /// `max_bytes` is the budget a put must fit; `next_version` hands out
    /// the number of the next write and is called at most once, only for a
    /// write that will be applied.
    ///
    /// # Errors
    ///
    /// [`Error::Rejected`] when a put does not fit the budget
    /// (`"store full: ..."`) or `next_version` is exhausted
    /// (`"store versions exhausted"`). A refused operation changes nothing.
    pub fn apply(
        &mut self,
        op: StoreOp,
        max_bytes: usize,
        next_version: impl FnMut() -> Option<u64>,
    ) -> Result<(u64, Vec<StoreEntry>)> {
        let entries = match op {
            StoreOp::Get { key } => self.get(&key).into_iter().collect(),
            StoreOp::Put { key, value } => vec![self.put(key, value, max_bytes, next_version)?],
            StoreOp::Delete { key } => self.delete(&key, next_version)?.into_iter().collect(),
            StoreOp::List => self.entries(),
        };
        Ok((self.revision, entries))
    }

    /// The entry for `key`, if it is set.
    pub fn get(&self, key: &StoreKey) -> Option<StoreEntry> {
        self.entries.get(key).map(|slot| StoreEntry {
            key: key.clone(),
            value: slot.value.clone(),
            version: slot.version,
        })
    }

    /// Set `key` to `value` and return the entry as written.
    ///
    /// # Errors
    ///
    /// [`Error::Rejected`] when the store's total after the put would
    /// exceed `max_bytes` (an overwrite counts the old entry's bytes as
    /// free), or when `next_version` is exhausted. Nothing changes then.
    pub fn put(
        &mut self,
        key: StoreKey,
        value: String,
        max_bytes: usize,
        mut next_version: impl FnMut() -> Option<u64>,
    ) -> Result<StoreEntry> {
        let old_cost = self
            .entries
            .get(&key)
            .map_or(0, |slot| entry_cost(&key, &slot.value));
        let others = self.bytes.saturating_sub(old_cost);
        let needed = entry_cost(&key, &value);
        if others.saturating_add(needed) > max_bytes {
            return Err(Error::Rejected(format!(
                "store full: the entry needs {needed} bytes and {} of {max_bytes} are free",
                max_bytes.saturating_sub(others)
            )));
        }
        let version = next_version().ok_or_else(exhausted)?;
        self.bytes = others.saturating_add(needed);
        self.revision = version;
        self.entries.insert(
            key.clone(),
            Slot {
                value: value.clone(),
                version,
            },
        );
        Ok(StoreEntry {
            key,
            value,
            version,
        })
    }

    /// Remove `key` and return the removed entry, with the version of its
    /// last write; `None`, taking no number, when the key is not set.
    ///
    /// # Errors
    ///
    /// [`Error::Rejected`] when the key is set and `next_version` is
    /// exhausted. Nothing changes then.
    pub fn delete(
        &mut self,
        key: &StoreKey,
        mut next_version: impl FnMut() -> Option<u64>,
    ) -> Result<Option<StoreEntry>> {
        let Entry::Occupied(occupied) = self.entries.entry(key.clone()) else {
            return Ok(None);
        };
        let version = next_version().ok_or_else(exhausted)?;
        let slot = occupied.remove();
        self.bytes = self.bytes.saturating_sub(entry_cost(key, &slot.value));
        self.revision = version;
        Ok(Some(StoreEntry {
            key: key.clone(),
            value: slot.value,
            version: slot.version,
        }))
    }

    /// Every entry, in ascending byte order of key.
    pub fn entries(&self) -> Vec<StoreEntry> {
        self.entries
            .iter()
            .map(|(key, slot)| StoreEntry {
                key: key.clone(),
                value: slot.value.clone(),
                version: slot.version,
            })
            .collect()
    }

    /// The number of the store's last write, 0 before the first.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The accounted size of every entry together (see [`entry_cost`]).
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// How many keys are set.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True when no key is set.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// The refusal when the version source has run out.
fn exhausted() -> Error {
    Error::Rejected("store versions exhausted".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::message::store_key as key;
    use crate::protocol::{Message, PartyId, Stored};
    use crate::testing::Rng;

    const BUDGET: usize = 16 * 1024;

    /// A version source counting up from `first`, and how often it was
    /// asked.
    struct Versions {
        next: u64,
        asked: usize,
    }

    impl Versions {
        fn from(first: u64) -> Self {
            Versions {
                next: first,
                asked: 0,
            }
        }

        fn source(&mut self) -> impl FnMut() -> Option<u64> + '_ {
            || {
                self.asked += 1;
                let v = self.next;
                self.next = self.next.checked_add(1)?;
                Some(v)
            }
        }
    }

    fn put(s: &mut Store, v: &mut Versions, k: &str, value: &str) -> Result<StoreEntry> {
        s.put(key(k), value.into(), BUDGET, v.source())
    }

    fn entry(k: &str, value: &str, version: u64) -> StoreEntry {
        StoreEntry {
            key: key(k),
            value: value.into(),
            version,
        }
    }

    fn rejected<T: fmt::Debug>(result: Result<T>) -> String {
        match result {
            Err(Error::Rejected(reason)) => reason,
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    // ----- cost accounting --------------------------------------------------

    #[test]
    fn json_len_matches_serde_json_for_random_text() {
        let mut alphabet: Vec<char> = (0u8..0x20).map(char::from).collect();
        alphabet.extend([
            '\u{7f}', '"', '\\', '/', ' ', 'a', 'Z', '9', 'é', 'λ', '😀', '\u{2028}', '\u{2029}',
            '<', '>', '&', '\'', '\u{feff}',
        ]);
        let mut rng = Rng::new(23);
        for i in 0..1000 {
            let text = if rng.chance(2) {
                rng.string(&alphabet, 64)
            } else {
                rng.text(64)
            };
            let expected = serde_json::to_string(&text).unwrap().len();
            assert_eq!(json_len(&text), expected, "iteration {i}: {text:?}");
        }
        assert_eq!(json_len(""), 2);
        assert_eq!(json_len("\u{1}"), 8);
        assert_eq!(json_len("\n"), 4);
    }

    #[test]
    fn entry_cost_counts_encoded_key_and_value_plus_the_overhead() {
        assert_eq!(entry_cost(&key("k"), ""), 3 + 2 + ENTRY_OVERHEAD);
        assert_eq!(entry_cost(&key("step"), "5"), 6 + 3 + ENTRY_OVERHEAD);
        assert_eq!(entry_cost(&key("k"), "\u{0}"), 3 + 8 + ENTRY_OVERHEAD);
        // The overhead covers the entry's own JSON around its key and value,
        // with the longest version and the comma between entries.
        let e = entry("k", "v", u64::MAX);
        let json = serde_json::to_string(&e).unwrap();
        assert!(
            json.len() < entry_cost(&e.key, &e.value),
            "{json} costs more than it is accounted"
        );
    }

    // ----- operations -------------------------------------------------------

    #[test]
    fn put_get_delete_and_list() {
        let mut s = Store::default();
        let mut v = Versions::from(1);
        assert!(s.is_empty());
        assert_eq!((s.revision(), s.bytes(), s.len()), (0, 0, 0));
        assert_eq!(s.get(&key("step")), None);

        assert_eq!(
            put(&mut s, &mut v, "step", "5").unwrap(),
            entry("step", "5", 1)
        );
        assert_eq!(put(&mut s, &mut v, "a", "").unwrap(), entry("a", "", 2));
        assert_eq!(
            put(&mut s, &mut v, "step", "6\nlines").unwrap(),
            entry("step", "6\nlines", 3)
        );
        assert_eq!(s.get(&key("step")), Some(entry("step", "6\nlines", 3)));
        assert_eq!(s.get(&key("a")), Some(entry("a", "", 2)), "empty is set");
        assert_eq!(s.len(), 2);
        assert_eq!(s.revision(), 3);
        assert_eq!(
            s.entries(),
            [entry("a", "", 2), entry("step", "6\nlines", 3)],
            "ascending key order"
        );
        assert_eq!(
            s.bytes(),
            entry_cost(&key("a"), "") + entry_cost(&key("step"), "6\nlines")
        );

        // A delete returns the removed entry with its last version and
        // takes the next number itself.
        assert_eq!(
            s.delete(&key("a"), v.source()).unwrap(),
            Some(entry("a", "", 2))
        );
        assert_eq!(s.revision(), 4);
        assert_eq!(s.get(&key("a")), None);
        assert_eq!(s.bytes(), entry_cost(&key("step"), "6\nlines"));
        assert_eq!(v.asked, 4);

        // Deleting an absent key, getting and listing take no number.
        assert_eq!(s.delete(&key("a"), v.source()).unwrap(), None);
        let _ = s.get(&key("step"));
        let _ = s.entries();
        assert_eq!((s.revision(), v.asked), (4, 4));
    }

    #[test]
    fn apply_dispatches_and_reports_the_revision() {
        let mut s = Store::default();
        let mut v = Versions::from(10);
        let get = |k: &str| StoreOp::Get { key: key(k) };
        assert_eq!(s.apply(get("k"), BUDGET, v.source()).unwrap(), (0, vec![]));
        assert_eq!(
            s.apply(
                StoreOp::Put {
                    key: key("k"),
                    value: "v".into()
                },
                BUDGET,
                v.source()
            )
            .unwrap(),
            (10, vec![entry("k", "v", 10)])
        );
        assert_eq!(
            s.apply(get("k"), BUDGET, v.source()).unwrap(),
            (10, vec![entry("k", "v", 10)])
        );
        assert_eq!(
            s.apply(StoreOp::List, BUDGET, v.source()).unwrap(),
            (10, vec![entry("k", "v", 10)])
        );
        assert_eq!(
            s.apply(StoreOp::Delete { key: key("k") }, BUDGET, v.source())
                .unwrap(),
            (11, vec![entry("k", "v", 10)])
        );
        assert_eq!(
            s.apply(StoreOp::Delete { key: key("k") }, BUDGET, v.source())
                .unwrap(),
            (11, vec![])
        );
        assert_eq!(
            s.apply(StoreOp::List, BUDGET, v.source()).unwrap(),
            (11, vec![])
        );
        assert_eq!(v.asked, 2);
    }

    #[test]
    fn versions_come_from_the_source_so_they_may_skip() {
        // The registry's counter is shared by every store, so one store sees
        // gaps; its revision is always the last number it was issued.
        let mut s = Store::default();
        let mut numbers = [5u64, 9, 40].into_iter();
        s.put(key("a"), "1".into(), BUDGET, || numbers.next())
            .unwrap();
        s.put(key("b"), "2".into(), BUDGET, || numbers.next())
            .unwrap();
        assert_eq!(s.entries(), [entry("a", "1", 5), entry("b", "2", 9)]);
        assert_eq!(s.revision(), 9);
        s.delete(&key("a"), || numbers.next()).unwrap();
        assert_eq!(s.revision(), 40);
    }

    #[test]
    fn an_exhausted_source_refuses_writes_and_changes_nothing() {
        let mut s = Store::default();
        let mut v = Versions::from(u64::MAX - 1);
        assert_eq!(
            put(&mut s, &mut v, "k", "v").unwrap(),
            entry("k", "v", u64::MAX - 1)
        );
        // The counter stops before u64::MAX: nothing is left to hand out.
        let before = s.clone();
        let never = || None;
        assert_eq!(
            rejected(s.put(key("k"), "w".into(), BUDGET, never)),
            "store versions exhausted"
        );
        assert_eq!(
            rejected(s.put(key("new"), "w".into(), BUDGET, never)),
            "store versions exhausted"
        );
        assert_eq!(
            rejected(s.delete(&key("k"), never)),
            "store versions exhausted"
        );
        assert_eq!(s, before);
        // What takes no number still works.
        assert_eq!(s.delete(&key("absent"), never).unwrap(), None);
        assert_eq!(
            s.apply(StoreOp::List, BUDGET, never).unwrap(),
            (u64::MAX - 1, vec![entry("k", "v", u64::MAX - 1)])
        );
    }

    // ----- the budget -------------------------------------------------------

    #[test]
    fn a_put_that_exactly_fills_the_budget_fits_and_one_byte_more_does_not() {
        let k = key("k");
        let first = entry_cost(&key("a"), "0123456789");
        for budget in [256, 1000, BUDGET] {
            let mut s = Store::default();
            let mut v = Versions::from(1);
            s.put(key("a"), "0123456789".into(), budget, v.source())
                .unwrap();
            // Fill the rest exactly: the value's JSON is its length plus 2.
            let exact = budget - first - entry_cost(&k, "");
            let value = "x".repeat(exact);
            let mut over = s.clone();
            s.put(k.clone(), value.clone(), budget, v.source())
                .unwrap_or_else(|e| panic!("budget {budget}: {e}"));
            assert_eq!(s.bytes(), budget, "budget {budget}");

            let reason = rejected(over.put(k.clone(), format!("{value}x"), budget, v.source()));
            assert_eq!(
                reason,
                format!(
                    "store full: the entry needs {} bytes and {} of {budget} are free",
                    entry_cost(&k, &value) + 1,
                    budget - first
                )
            );
            assert_eq!(over.bytes(), first, "a refused put changes nothing");
            assert_eq!(over.get(&k), None);
            // A full store refuses even the smallest new entry...
            let reason = rejected(s.put(key("b"), String::new(), budget, v.source()));
            assert!(reason.contains("and 0 of"), "{reason}");
            // ...but an overwrite of the same size or smaller always fits.
            s.put(k.clone(), "y".repeat(exact), budget, v.source())
                .unwrap();
            s.put(k.clone(), "y".into(), budget, v.source()).unwrap();
            assert_eq!(s.bytes(), first + entry_cost(&k, "y"));
        }
    }

    #[test]
    fn overwrites_recompute_the_bytes_as_they_grow_and_shrink() {
        let mut s = Store::default();
        let mut v = Versions::from(1);
        put(&mut s, &mut v, "other", "o").unwrap();
        let base = s.bytes();
        let long = "long".repeat(100);
        for value in ["", "short", long.as_str(), "\"\\\n", "", "é"] {
            put(&mut s, &mut v, "k", value).unwrap();
            assert_eq!(s.bytes(), base + entry_cost(&key("k"), value), "{value:?}");
        }
        s.delete(&key("k"), v.source()).unwrap();
        assert_eq!(s.bytes(), base);
        s.delete(&key("other"), v.source()).unwrap();
        assert_eq!(s.bytes(), 0);
    }

    #[test]
    fn an_entry_larger_than_the_whole_budget_is_refused() {
        let mut s = Store::default();
        let mut v = Versions::from(1);
        let value = "x".repeat(BUDGET);
        let reason = rejected(s.put(key("k"), value.clone(), BUDGET, v.source()));
        assert_eq!(
            reason,
            format!(
                "store full: the entry needs {} bytes and {BUDGET} of {BUDGET} are free",
                entry_cost(&key("k"), &value)
            )
        );
        // Escaping counts: 3000 control characters encode to 18000 bytes.
        let reason = rejected(s.put(key("k"), "\u{1}".repeat(3000), BUDGET, v.source()));
        assert!(reason.starts_with("store full"), "{reason}");
        assert!(s.is_empty());
        assert_eq!((s.revision(), v.asked), (0, 0));
    }

    #[test]
    fn a_random_full_store_fits_its_reply_in_the_budget_plus_the_overhead() {
        let mut rng = Rng::new(31);
        for i in 0..60 {
            let budget = *rng.pick(&[256, 1024, BUDGET, 32 * 1024]);
            let mut s = Store::default();
            // Versions of twenty digits, the longest a u64 has.
            let mut v = Versions::from(10_000_000_000_000_000_000);
            let mut refusals = 0;
            while refusals < 20 {
                let value = if rng.chance(3) {
                    rng.string(&['\u{1}', '"', '\n', 'x'], 400)
                } else {
                    rng.text(200)
                };
                match s.put(rng.store_key(), value, budget, v.source()) {
                    Ok(_) => {}
                    Err(Error::Rejected(_)) => refusals += 1,
                    Err(e) => panic!("iteration {i}: {e}"),
                }
            }
            assert!(s.bytes() <= budget, "iteration {i}");
            let accounted: usize = s
                .entries()
                .iter()
                .map(|e| entry_cost(&e.key, &e.value))
                .sum();
            assert_eq!(s.bytes(), accounted, "iteration {i}");
            let reply = Message::Stored(Stored {
                client: Some(PartyId(u64::MAX)),
                revision: u64::MAX,
                entries: s.entries(),
            });
            let wire = serde_json::to_vec(&reply).unwrap();
            assert!(
                wire.len() <= budget + REPLY_OVERHEAD,
                "iteration {i}: {} entries, {} bytes accounted, a reply of {} bytes exceeds {budget} + {REPLY_OVERHEAD}",
                s.len(),
                s.bytes(),
                wire.len()
            );
        }
    }

    #[test]
    fn random_operations_agree_with_a_plain_map() {
        let mut rng = Rng::new(37);
        let mut s = Store::default();
        let mut v = Versions::from(1);
        let mut model: BTreeMap<StoreKey, (String, u64)> = BTreeMap::new();
        let keys: Vec<StoreKey> = (0..12).map(|_| rng.store_key()).collect();
        let budget = 2048;
        for i in 0..3000 {
            let k = rng.pick(&keys).clone();
            let op = match rng.below(4) {
                0 => StoreOp::Get { key: k.clone() },
                1 | 2 => StoreOp::Put {
                    key: k.clone(),
                    value: rng.text(120),
                },
                _ => StoreOp::Delete { key: k.clone() },
            };
            let op = if rng.chance(10) { StoreOp::List } else { op };
            let before = (s.clone(), v.next);
            let result = s.apply(op.clone(), budget, v.source());
            match (&op, result) {
                (StoreOp::Put { key, value }, Ok((rev, entries))) => {
                    assert_eq!(entries, [entry(key.as_str(), value, rev)], "iteration {i}");
                    model.insert(key.clone(), (value.clone(), rev));
                }
                (StoreOp::Put { .. }, Err(Error::Rejected(reason))) => {
                    assert!(reason.starts_with("store full"), "iteration {i}: {reason}");
                    assert_eq!((s.clone(), v.next), before, "iteration {i}");
                }
                (StoreOp::Delete { key }, Ok((_, entries))) => {
                    let removed = model.remove(key);
                    let expected: Vec<StoreEntry> = removed
                        .map(|(value, version)| entry(key.as_str(), &value, version))
                        .into_iter()
                        .collect();
                    assert_eq!(entries, expected, "iteration {i}");
                }
                (StoreOp::Get { key }, Ok((_, entries))) => {
                    let expected: Vec<StoreEntry> = model
                        .get(key)
                        .map(|(value, version)| entry(key.as_str(), value, *version))
                        .into_iter()
                        .collect();
                    assert_eq!(entries, expected, "iteration {i}");
                }
                (StoreOp::List, Ok((_, entries))) => {
                    let expected: Vec<StoreEntry> = model
                        .iter()
                        .map(|(key, (value, version))| entry(key.as_str(), value, *version))
                        .collect();
                    assert_eq!(entries, expected, "iteration {i}");
                }
                (op, other) => panic!("iteration {i}: {} gave {other:?}", op.kind()),
            }
            assert_eq!(s.revision(), v.next - 1, "iteration {i}");
            assert!(s.revision() >= before.0.revision(), "iteration {i}");
            assert!(s.bytes() <= budget, "iteration {i}");
            assert_eq!(s.len(), model.len(), "iteration {i}");
        }
    }

    #[test]
    fn debug_shows_counts_and_never_a_key_or_value() {
        let mut s = Store::default();
        let mut v = Versions::from(1);
        put(&mut s, &mut v, "secret-key", "secret-value").unwrap();
        let debug = format!("{s:?}");
        assert!(!debug.contains("secret"), "{debug}");
        assert_eq!(
            debug,
            format!(
                "Store {{ entries: 1, bytes: {}, revision: 1 }}",
                entry_cost(&key("secret-key"), "secret-value")
            )
        );
        assert!(!format!("{s:#?}").contains("secret"));
    }
}
