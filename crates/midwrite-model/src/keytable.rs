//! The visited-state table of the explicit-state search (Phase 9).
//!
//! A state is identified by its *key*: the sequence of interned fragment
//! integers of its canonical encoding (see [`crate::state::Interner`]). Keys
//! live back to back in one arena as LEB128 varints (one byte per fragment
//! while there are fewer than 128 distinct fragments, two below 16384, ...);
//! the table holds 4-byte references into it. Lookups compare the encoded
//! keys byte for byte, never just their hashes, so a hash collision cannot
//! make a reachable state disappear, and the varint encoding is injective on
//! sequences. Per stored state the table needs the key, 4 bytes of offset and
//! between 4 and 8 bytes of slot.

use crate::fasthash::SEED;

pub struct KeyTable {
    arena: Vec<u8>,
    /// `starts[id]..starts[id + 1]` is the key of state `id`.
    starts: Vec<u32>,
    /// `id + 1` of the state in this slot, `0` when empty.
    slots: Vec<u32>,
    mask: usize,
    scratch: Vec<u8>,
}

impl Default for KeyTable {
    fn default() -> Self {
        Self::new()
    }
}

fn hash(key: &[u8]) -> u64 {
    let mut h: u64 = 0;
    for x in key {
        h = (h.rotate_left(5) ^ u64::from(*x)).wrapping_mul(SEED);
    }
    h ^ (h >> 29)
}

/// LEB128: 7 bits per byte, the high bit says that more follow.
fn encode(key: &[u32], out: &mut Vec<u8>) {
    out.clear();
    for &x in key {
        let mut v = x;
        loop {
            let byte = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                out.push(byte);
                break;
            }
            out.push(byte | 0x80);
        }
    }
}

impl KeyTable {
    pub fn new() -> Self {
        Self {
            arena: Vec::new(),
            starts: vec![0],
            slots: vec![0; 1024],
            mask: 1023,
            scratch: Vec::new(),
        }
    }

    /// Number of stored keys.
    pub fn len(&self) -> usize {
        self.starts.len() - 1
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn key(&self, id: usize) -> &[u8] {
        &self.arena[self.starts[id] as usize..self.starts[id + 1] as usize]
    }

    /// The id of `key`, and whether it was new. Ids are dense and assigned
    /// in order of first insertion.
    pub fn intern(&mut self, key: &[u32]) -> (u32, bool) {
        let mut bytes = std::mem::take(&mut self.scratch);
        encode(key, &mut bytes);
        let r = self.intern_bytes(&bytes);
        self.scratch = bytes;
        r
    }

    fn intern_bytes(&mut self, key: &[u8]) -> (u32, bool) {
        self.intern_hashed(key, hash(key))
    }

    /// `intern_bytes` with the hash given: tests force equal hashes with it.
    fn intern_hashed(&mut self, key: &[u8], h: u64) -> (u32, bool) {
        let mut i = (h as usize) & self.mask;
        loop {
            let slot = self.slots[i];
            if slot == 0 {
                break;
            }
            if self.key(slot as usize - 1) == key {
                return (slot - 1, false);
            }
            i = (i + 1) & self.mask;
        }
        let id = self.len() as u32;
        assert!(
            self.arena.len() + key.len() < u32::MAX as usize,
            "the visited-state arena is full"
        );
        self.arena.extend_from_slice(key);
        self.starts.push(self.arena.len() as u32);
        self.slots[i] = id + 1;
        if self.len() * 10 > self.slots.len() * 7 {
            self.grow();
        }
        (id, true)
    }

    fn grow(&mut self) {
        let size = self.slots.len() * 2;
        self.slots = vec![0; size];
        self.mask = size - 1;
        for id in 0..self.len() {
            let mut i = (hash(self.key(id)) as usize) & self.mask;
            while self.slots[i] != 0 {
                i = (i + 1) & self.mask;
            }
            self.slots[i] = id as u32 + 1;
        }
    }

    /// Bytes of the encoded keys.
    pub fn arena_len(&self) -> usize {
        self.arena.len()
    }

    /// Memory held by the table, in bytes.
    pub fn memory_bytes(&self) -> u64 {
        (self.arena.capacity() + (self.starts.capacity() + self.slots.len()) * 4) as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_keys_get_one_id_and_different_keys_never_share_one() {
        let mut t = KeyTable::new();
        let mut ids = std::collections::BTreeMap::new();
        for round in 0..3 {
            for a in 0..300u32 {
                for len in 0..4usize {
                    let key: Vec<u32> = (0..len as u32).map(|j| a.wrapping_mul(7) ^ j).collect();
                    let (id, new) = t.intern(&key);
                    match ids.insert(key.clone(), id) {
                        None => assert!(new && round == 0),
                        Some(previous) => assert!(!new && previous == id),
                    }
                }
            }
        }
        assert_eq!(t.len(), ids.len());
        // Prefixes and extensions are different keys.
        let (a, _) = t.intern(&[1, 2]);
        let (b, _) = t.intern(&[1, 2, 3]);
        let (c, _) = t.intern(&[1]);
        assert!(a != b && b != c && a != c);
    }

    #[test]
    fn colliding_hashes_do_not_merge_states() {
        // Every key gets the same full 64-bit hash, so all of them share one
        // probe sequence: only the byte comparison tells them apart. (600
        // keys stay below the growth threshold, which rehashes with `hash`.)
        let mut t = KeyTable::new();
        let keys: Vec<Vec<u8>> = (0..600u32)
            .map(|i| {
                let mut b = Vec::new();
                encode(&[i % 7, i, i ^ 0x55], &mut b);
                b
            })
            .collect();
        for (i, k) in keys.iter().enumerate() {
            assert_eq!(t.intern_hashed(k, 0xC0FFEE), (i as u32, true), "key {i}");
        }
        for (i, k) in keys.iter().enumerate() {
            assert_eq!(t.intern_hashed(k, 0xC0FFEE), (i as u32, false), "key {i}");
        }
        // A key that is a prefix of a colliding key is still a different key.
        let (p, new) = t.intern_hashed(&keys[1][..1], 0xC0FFEE);
        assert!(new && p == 600);
    }

    #[test]
    fn slot_collisions_do_not_merge_states() {
        // Many keys share probe sequences in the default table and through
        // its growth: each is still found as itself.
        let mut t = KeyTable::new();
        for i in 0..5000u32 {
            let (id, new) = t.intern(&[i, i.rotate_left(13)]);
            assert!(new);
            assert_eq!(id, i);
        }
        for i in 0..5000u32 {
            assert_eq!(t.intern(&[i, i.rotate_left(13)]), (i, false));
        }
    }
}
