//! Lock-free shared transposition table.
//!
//! Each slot is two `AtomicU64`s: `key ^ data` and `data` (Hyatt's XOR trick),
//! so threads can race on a slot without locks and a torn read is simply seen
//! as a miss. Memory comes from `alloc_zeroed` (calloc), so even multi-GB
//! tables are "allocated" instantly and paged in lazily by the OS.

use std::sync::atomic::{AtomicU8, AtomicU64, Ordering::Relaxed};

use crate::types::Move;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Bound {
    Exact = 1,
    Lower = 2,
    Upper = 3,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct TtEntry {
    pub mv: Move,
    pub score: i32,
    pub eval: i32,
    pub depth: i32,
    pub bound: Bound,
    pub age: u8,
}

impl TtEntry {
    /// data layout: move:16 | score:16 | eval:16 | depth:8 | bound:2 | age:6
    #[inline(always)]
    pub fn pack(&self) -> u64 {
        self.mv.0 as u64
            | (self.score as i16 as u16 as u64) << 16
            | (self.eval as i16 as u16 as u64) << 32
            | (self.depth.clamp(-128, 127) as i8 as u8 as u64) << 48
            | (self.bound as u64) << 56
            | ((self.age & 63) as u64) << 58
    }

    #[inline(always)]
    pub fn unpack(d: u64) -> Option<TtEntry> {
        let bound = match (d >> 56) & 3 {
            1 => Bound::Exact,
            2 => Bound::Lower,
            3 => Bound::Upper,
            _ => return None,
        };
        Some(TtEntry {
            mv: Move(d as u16),
            score: (d >> 16) as u16 as i16 as i32,
            eval: (d >> 32) as u16 as i16 as i32,
            depth: (d >> 48) as u8 as i8 as i32,
            bound,
            age: (d >> 58) as u8,
        })
    }
}

#[repr(C, align(16))]
struct Slot {
    key: AtomicU64,
    data: AtomicU64,
}

pub struct TranspositionTable {
    slots: Box<[Slot]>,
    mask: usize,
    age: AtomicU8,
}

impl TranspositionTable {
    /// Create a table of (at most) `mb` megabytes, rounded down to a power of two.
    pub fn new(mb: usize) -> Self {
        let bytes = mb.max(1) * 1024 * 1024;
        let n = 1usize << (bytes / size_of::<Slot>()).max(1).ilog2();
        TranspositionTable { slots: zeroed_slots(n), mask: n - 1, age: AtomicU8::new(0) }
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    pub fn size_mb(&self) -> usize {
        self.slots.len() * size_of::<Slot>() / (1024 * 1024)
    }

    pub fn clear(&self) {
        for s in self.slots.iter() {
            s.key.store(0, Relaxed);
            s.data.store(0, Relaxed);
        }
    }

    pub fn new_search(&self) {
        self.age.store(self.age.load(Relaxed).wrapping_add(1) & 63, Relaxed);
    }

    #[inline(always)]
    pub fn age(&self) -> u8 {
        self.age.load(Relaxed)
    }

    #[inline(always)]
    fn slot(&self, hash: u64) -> &Slot {
        // SAFETY: mask = len - 1 with len a power of two.
        unsafe { self.slots.get_unchecked(hash as usize & self.mask) }
    }

    #[inline(always)]
    pub fn probe(&self, hash: u64) -> Option<TtEntry> {
        let s = self.slot(hash);
        let data = s.data.load(Relaxed);
        let key = s.key.load(Relaxed);
        if key ^ data == hash { TtEntry::unpack(data) } else { None }
    }

    /// Hint the CPU to start fetching the slot for `hash` (used right after
    /// making a move, before the child node probes the table).
    #[inline(always)]
    pub fn prefetch(&self, hash: u64) {
        #[cfg(target_arch = "x86_64")]
        // SAFETY: prefetch is a pure hint; the pointer is in-bounds.
        unsafe {
            use std::arch::x86_64::{_MM_HINT_T0, _mm_prefetch};
            _mm_prefetch::<_MM_HINT_T0>(self.slot(hash) as *const Slot as *const i8);
        }
        #[cfg(not(target_arch = "x86_64"))]
        let _ = hash;
    }

    #[inline(always)]
    pub fn store(&self, hash: u64, mv: Move, score: i32, eval: i32, depth: i32, bound: Bound) {
        let s = self.slot(hash);
        let old_data = s.data.load(Relaxed);
        let old_key = s.key.load(Relaxed) ^ old_data;
        let age = self.age();
        let mut mv = mv;
        if let Some(old) = TtEntry::unpack(old_data) {
            if old_key == hash {
                if mv.is_null() {
                    mv = old.mv; // keep the best move we knew about
                }
                if bound != Bound::Exact && depth + 3 < old.depth && old.age == age {
                    return; // much deeper result for the same position: keep it
                }
            } else if old.age == age && depth < old.depth - 2 && bound != Bound::Exact {
                return; // keep deeper current-search entries of other positions
            }
        }
        let data = TtEntry { mv, score, eval, depth, bound, age }.pack();
        s.key.store(hash ^ data, Relaxed);
        s.data.store(data, Relaxed);
    }

    /// Insert a raw (key, data) pair, e.g. from the on-disk cache. Only
    /// replaces an existing entry if the incoming one is at least as deep.
    pub fn insert_raw(&self, hash: u64, data: u64) {
        let Some(mut e) = TtEntry::unpack(data) else { return };
        let s = self.slot(hash);
        let old_data = s.data.load(Relaxed);
        if let Some(old) = TtEntry::unpack(old_data)
            && old.depth > e.depth
        {
            return;
        }
        e.age = self.age();
        let data = e.pack();
        s.key.store(hash ^ data, Relaxed);
        s.data.store(data, Relaxed);
    }

    /// Iterate over all valid entries as (hash, packed data).
    pub fn entries(&self) -> impl Iterator<Item = (u64, u64)> + '_ {
        self.slots.iter().filter_map(|s| {
            let data = s.data.load(Relaxed);
            let key = s.key.load(Relaxed) ^ data;
            TtEntry::unpack(data).map(|_| (key, data))
        })
    }

    /// Permille of a sample of slots that are filled in the current search.
    pub fn hashfull(&self) -> usize {
        let n = self.slots.len().min(1000);
        let age = self.age();
        self.slots[..n].iter().filter(|s| TtEntry::unpack(s.data.load(Relaxed)).is_some_and(|e| e.age == age)).count()
            * 1000
            / n
    }
}

/// Allocate `n` zeroed slots via `alloc_zeroed` without touching the memory.
fn zeroed_slots(n: usize) -> Box<[Slot]> {
    use std::alloc::{Layout, alloc_zeroed, handle_alloc_error};
    let layout = Layout::array::<Slot>(n).expect("TT size overflow");
    // SAFETY: an all-zero bit pattern is a valid `Slot` (two AtomicU64 zeros),
    // and the pointer comes from the global allocator with `Slot`'s layout,
    // which is exactly what `Box<[Slot]>` will free it with.
    unsafe {
        let ptr = alloc_zeroed(layout) as *mut Slot;
        if ptr.is_null() {
            handle_alloc_error(layout);
        }
        Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr, n))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_roundtrip() {
        for (score, eval, depth, bound) in
            [(0, 0, 0, Bound::Exact), (-31_000, 512, 63, Bound::Lower), (30_999, -2000, -1, Bound::Upper)]
        {
            let e = TtEntry { mv: Move(0xBEEF), score, eval, depth, bound, age: 42 };
            assert_eq!(TtEntry::unpack(e.pack()), Some(e));
        }
        assert_eq!(TtEntry::unpack(0), None);
    }

    #[test]
    fn store_probe_and_size() {
        let tt = TranspositionTable::new(1);
        assert!(tt.len().is_power_of_two());
        assert_eq!(tt.size_mb(), 1);
        assert_eq!(TranspositionTable::new(3).size_mb(), 2);
        let h = 0x1234_5678_9abc_def0;
        assert!(tt.probe(h).is_none());
        tt.store(h, Move(77), 150, 20, 5, Bound::Lower);
        let e = tt.probe(h).unwrap();
        assert_eq!((e.mv, e.score, e.eval, e.depth, e.bound), (Move(77), 150, 20, 5, Bound::Lower));
        // A different key mapping to the same slot is a miss.
        assert!(tt.probe(h ^ (1 << 60)).is_none());
        // Shallow non-exact result does not clobber a much deeper one.
        tt.store(h, Move(5), 1, 1, 1, Bound::Upper);
        assert_eq!(tt.probe(h).unwrap().depth, 5);
        tt.clear();
        assert!(tt.probe(h).is_none());
    }
}
