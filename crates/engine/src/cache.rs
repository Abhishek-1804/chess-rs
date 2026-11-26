//! Persistent position cache: survives across games and program runs.
//!
//! Two layers, both keyed by the (deterministic) Zobrist hash:
//! * **root records** — the result of every completed bot search (best move,
//!   score, depth). If the bot meets a position it already searched at least
//!   as deeply, it can answer instantly.
//! * **deep TT entries** — transposition-table entries above a depth threshold,
//!   used to warm the TT at start-up so new searches start from prior work.
//!
//! The file is versioned: if the engine's evaluation or search changes, bump
//! [`CACHE_VERSION`] and stale caches are discarded automatically. Writes go
//! to a temp file and are renamed into place, so a crash never corrupts it.

use std::collections::HashMap;
use std::fs;
use std::hash::{BuildHasherDefault, Hasher};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use crate::tt::{TranspositionTable, TtEntry};
use crate::types::Move;

const MAGIC: &[u8; 8] = b"CHSRSCH\0";
/// Bump when evaluation, search scores, or the hash keys change.
pub const CACHE_VERSION: u32 = 3;
/// TT entries shallower than this are not worth persisting.
pub const MIN_PERSIST_DEPTH: i32 = 6;
/// Upper bound on persisted TT entries (16 bytes each => 128 MiB).
pub const MAX_TT_ENTRIES: usize = 8 << 20;

/// Zobrist keys are already uniformly random: hashing them again is waste.
#[derive(Default)]
pub struct IdentityHasher(u64);

impl Hasher for IdentityHasher {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = self.0.rotate_left(8) ^ b as u64;
        }
    }
    fn write_u64(&mut self, n: u64) {
        self.0 = n;
    }
}

pub type ZobristMap<V> = HashMap<u64, V, BuildHasherDefault<IdentityHasher>>;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct RootRecord {
    pub mv: Move,
    /// Side-to-move relative score.
    pub score: i32,
    pub depth: u32,
}

impl RootRecord {
    fn pack(&self) -> u64 {
        self.mv.0 as u64 | (self.score as i16 as u16 as u64) << 16 | (self.depth.min(255) as u64) << 32
    }
    fn unpack(d: u64) -> RootRecord {
        RootRecord { mv: Move(d as u16), score: (d >> 16) as u16 as i16 as i32, depth: ((d >> 32) & 0xFF) as u32 }
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct CacheStats {
    pub root_entries: usize,
    pub tt_entries: usize,
    pub hits: u64,
}

pub struct PositionCache {
    path: Option<PathBuf>,
    roots: ZobristMap<RootRecord>,
    tt_entries: ZobristMap<u64>,
    hits: u64,
    dirty: bool,
}

impl PositionCache {
    /// An in-memory cache that is never written to disk.
    pub fn in_memory() -> Self {
        PositionCache {
            path: None,
            roots: ZobristMap::default(),
            tt_entries: ZobristMap::default(),
            hits: 0,
            dirty: false,
        }
    }

    /// Load from `path`. A missing, corrupt or out-of-date file yields an
    /// empty cache (which will overwrite it on the next save).
    pub fn open(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let mut cache = Self::in_memory();
        if let Ok(f) = fs::File::open(&path)
            && let Ok((roots, tt)) = read_cache(BufReader::with_capacity(1 << 20, f))
        {
            cache.roots = roots;
            cache.tt_entries = tt;
        }
        cache.path = Some(path);
        cache
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn stats(&self) -> CacheStats {
        CacheStats { root_entries: self.roots.len(), tt_entries: self.tt_entries.len(), hits: self.hits }
    }

    /// Copy persisted deep entries into a live transposition table.
    pub fn warm(&self, tt: &TranspositionTable) {
        for (&k, &d) in &self.tt_entries {
            tt.insert_raw(k, d);
        }
    }

    /// A previous search of this position at depth >= `min_depth`, if any.
    pub fn lookup(&mut self, hash: u64, min_depth: u32) -> Option<RootRecord> {
        let r = self.roots.get(&hash).copied().filter(|r| r.depth >= min_depth)?;
        self.hits += 1;
        Some(r)
    }

    /// Remember a completed search (keeps the deeper record on conflict).
    pub fn record(&mut self, hash: u64, rec: RootRecord) {
        let e = self.roots.entry(hash).or_insert(rec);
        if rec.depth >= e.depth {
            *e = rec;
        }
        self.dirty = true;
    }

    /// Pull the deep entries of a live TT into the persistent set.
    pub fn absorb(&mut self, tt: &TranspositionTable) {
        for (k, d) in tt.entries() {
            let Some(e) = TtEntry::unpack(d) else { continue };
            if e.depth < MIN_PERSIST_DEPTH {
                continue;
            }
            let slot = self.tt_entries.entry(k).or_insert(d);
            if TtEntry::unpack(*slot).is_none_or(|old| e.depth >= old.depth) {
                *slot = d;
            }
            self.dirty = true;
        }
        if self.tt_entries.len() > MAX_TT_ENTRIES {
            let mut v: Vec<(u64, u64)> = self.tt_entries.drain().collect();
            v.sort_unstable_by_key(|&(_, d)| std::cmp::Reverse(TtEntry::unpack(d).map_or(0, |e| e.depth)));
            v.truncate(MAX_TT_ENTRIES);
            self.tt_entries.extend(v);
        }
    }

    pub fn clear(&mut self) {
        self.roots.clear();
        self.tt_entries.clear();
        self.dirty = true;
    }

    /// Atomically write the cache to its path (no-op for in-memory caches).
    pub fn save(&mut self) -> io::Result<()> {
        let Some(path) = self.path.clone() else { return Ok(()) };
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("tmp");
        {
            let mut w = BufWriter::with_capacity(1 << 20, fs::File::create(&tmp)?);
            write_cache(&mut w, &self.roots, &self.tt_entries)?;
            w.flush()?;
            w.get_ref().sync_all()?;
        }
        fs::rename(&tmp, &path)?;
        self.dirty = false;
        Ok(())
    }
}

/// FNV-1a over the payload, to detect truncated or corrupted files.
struct Checksum(u64);
impl Checksum {
    fn new() -> Self {
        Checksum(0xcbf2_9ce4_8422_2325)
    }
    fn add(&mut self, v: u64) {
        for b in v.to_le_bytes() {
            self.0 = (self.0 ^ b as u64).wrapping_mul(0x0100_0000_01b3);
        }
    }
}

fn write_cache(w: &mut impl Write, roots: &ZobristMap<RootRecord>, tt: &ZobristMap<u64>) -> io::Result<()> {
    let mut sum = Checksum::new();
    w.write_all(MAGIC)?;
    w.write_all(&CACHE_VERSION.to_le_bytes())?;
    w.write_all(&(roots.len() as u64).to_le_bytes())?;
    w.write_all(&(tt.len() as u64).to_le_bytes())?;
    for (&k, r) in roots {
        let d = r.pack();
        w.write_all(&k.to_le_bytes())?;
        w.write_all(&d.to_le_bytes())?;
        sum.add(k);
        sum.add(d);
    }
    for (&k, &d) in tt {
        w.write_all(&k.to_le_bytes())?;
        w.write_all(&d.to_le_bytes())?;
        sum.add(k);
        sum.add(d);
    }
    w.write_all(&sum.0.to_le_bytes())
}

type Loaded = (ZobristMap<RootRecord>, ZobristMap<u64>);

fn read_cache(mut r: impl Read) -> io::Result<Loaded> {
    let bad = |m: &str| io::Error::new(io::ErrorKind::InvalidData, m.to_string());
    let mut magic = [0u8; 8];
    r.read_exact(&mut magic)?;
    if &magic != MAGIC {
        return Err(bad("not a chess-rs cache file"));
    }
    let mut b4 = [0u8; 4];
    r.read_exact(&mut b4)?;
    if u32::from_le_bytes(b4) != CACHE_VERSION {
        return Err(bad("cache version mismatch"));
    }
    let mut u64_ = || -> io::Result<u64> {
        let mut b = [0u8; 8];
        r.read_exact(&mut b)?;
        Ok(u64::from_le_bytes(b))
    };
    let n_roots = u64_()? as usize;
    let n_tt = u64_()? as usize;
    if n_roots > 1 << 30 || n_tt > 1 << 30 {
        return Err(bad("implausible cache size"));
    }
    let mut sum = Checksum::new();
    let mut roots = ZobristMap::with_capacity_and_hasher(n_roots, Default::default());
    for _ in 0..n_roots {
        let (k, d) = (u64_()?, u64_()?);
        sum.add(k);
        sum.add(d);
        roots.insert(k, RootRecord::unpack(d));
    }
    let mut tt = ZobristMap::with_capacity_and_hasher(n_tt, Default::default());
    for _ in 0..n_tt {
        let (k, d) = (u64_()?, u64_()?);
        sum.add(k);
        sum.add(d);
        tt.insert(k, d);
    }
    if u64_()? != sum.0 {
        return Err(bad("cache checksum mismatch"));
    }
    Ok((roots, tt))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tt::Bound;

    fn tmp_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("chess-rs-test-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir.join("cache.bin")
    }

    #[test]
    fn save_load_roundtrip_and_warm() {
        let path = tmp_path("roundtrip");
        let tt = TranspositionTable::new(1);
        tt.store(0xAAAA_0000_0000_0001, Move(123), 55, 10, 9, Bound::Exact);
        tt.store(0xBBBB_0000_0000_0002, Move(124), -20, 3, 2, Bound::Lower); // too shallow
        let mut c = PositionCache::open(&path);
        assert_eq!(c.stats(), CacheStats::default());
        c.record(42, RootRecord { mv: Move(7), score: -310, depth: 12 });
        c.record(42, RootRecord { mv: Move(8), score: 0, depth: 4 }); // shallower: ignored
        c.absorb(&tt);
        c.save().unwrap();

        let mut c2 = PositionCache::open(&path);
        assert_eq!(c2.stats().root_entries, 1);
        assert_eq!(c2.stats().tt_entries, 1);
        assert_eq!(c2.lookup(42, 12), Some(RootRecord { mv: Move(7), score: -310, depth: 12 }));
        assert_eq!(c2.lookup(42, 13), None);
        assert_eq!(c2.stats().hits, 1);

        let fresh = TranspositionTable::new(1);
        c2.warm(&fresh);
        let e = fresh.probe(0xAAAA_0000_0000_0001).unwrap();
        assert_eq!((e.mv, e.score, e.depth, e.bound), (Move(123), 55, 9, Bound::Exact));
        assert!(fresh.probe(0xBBBB_0000_0000_0002).is_none());
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn corrupt_file_is_ignored() {
        let path = tmp_path("corrupt");
        let mut c = PositionCache::open(&path);
        c.record(1, RootRecord { mv: Move(1), score: 1, depth: 1 });
        c.save().unwrap();
        let mut bytes = fs::read(&path).unwrap();
        let n = bytes.len();
        bytes[n - 12] ^= 0xFF; // flip payload bits
        fs::write(&path, &bytes).unwrap();
        assert_eq!(PositionCache::open(&path).stats().root_entries, 0);
        fs::write(&path, b"garbage").unwrap();
        assert_eq!(PositionCache::open(&path).stats().root_entries, 0);
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }
}
