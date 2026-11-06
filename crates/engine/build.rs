//! Build script: generates every static lookup table the engine needs.
//!
//! Magic numbers are found here with a deterministic PRNG, and the full
//! sliding-attack table is emitted as a `static`, so the engine has zero
//! runtime initialisation and the tables live in read-only memory.

#![allow(clippy::needless_range_loop)] // table generation reads best with explicit indices

use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;

const ROOK_DIRS: [(i32, i32); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];
const BISHOP_DIRS: [(i32, i32); 4] = [(1, 1), (1, -1), (-1, 1), (-1, -1)];

fn on_board(r: i32, f: i32) -> bool {
    (0..8).contains(&r) && (0..8).contains(&f)
}

fn bit(r: i32, f: i32) -> u64 {
    1u64 << (r * 8 + f)
}

/// Slow reference sliding attacks (rays stop at, and include, the first blocker).
fn slide(sq: usize, occ: u64, dirs: &[(i32, i32)]) -> u64 {
    let (r0, f0) = ((sq / 8) as i32, (sq % 8) as i32);
    let mut att = 0;
    for &(dr, df) in dirs {
        let (mut r, mut f) = (r0 + dr, f0 + df);
        while on_board(r, f) {
            att |= bit(r, f);
            if occ & bit(r, f) != 0 {
                break;
            }
            r += dr;
            f += df;
        }
    }
    att
}

/// Relevant occupancy mask: rays excluding the board edge in each direction.
fn relevant_mask(sq: usize, dirs: &[(i32, i32)]) -> u64 {
    let (r0, f0) = ((sq / 8) as i32, (sq % 8) as i32);
    let mut m = 0;
    for &(dr, df) in dirs {
        let (mut r, mut f) = (r0 + dr, f0 + df);
        while on_board(r + dr, f + df) {
            m |= bit(r, f);
            r += dr;
            f += df;
        }
    }
    m
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn sparse(&mut self) -> u64 {
        self.next() & self.next() & self.next()
    }
}

struct MagicEntry {
    mask: u64,
    magic: u64,
    shift: u32,
    table: Vec<u64>,
}

fn find_magic(sq: usize, dirs: &[(i32, i32)], rng: &mut Rng) -> MagicEntry {
    let mask = relevant_mask(sq, dirs);
    let bits = mask.count_ones();
    let n = 1usize << bits;
    let mut occs = Vec::with_capacity(n);
    let mut atts = Vec::with_capacity(n);
    // Carry-rippler enumeration of every subset of the mask.
    let mut sub = 0u64;
    loop {
        occs.push(sub);
        atts.push(slide(sq, sub, dirs));
        sub = sub.wrapping_sub(mask) & mask;
        if sub == 0 {
            break;
        }
    }
    let shift = 64 - bits;
    let mut table = vec![0u64; n];
    let mut epoch = vec![0u32; n];
    let mut attempt = 0u32;
    loop {
        let magic = rng.sparse();
        if (mask.wrapping_mul(magic) >> 56).count_ones() < 6 {
            continue;
        }
        attempt += 1;
        let mut ok = true;
        for i in 0..n {
            let idx = (occs[i].wrapping_mul(magic) >> shift) as usize;
            if epoch[idx] != attempt {
                epoch[idx] = attempt;
                table[idx] = atts[i];
            } else if table[idx] != atts[i] {
                ok = false;
                break;
            }
        }
        if ok {
            return MagicEntry { mask, magic, shift, table };
        }
    }
}

fn leaper(sq: usize, deltas: &[(i32, i32)]) -> u64 {
    let (r0, f0) = ((sq / 8) as i32, (sq % 8) as i32);
    deltas.iter().filter(|&&(dr, df)| on_board(r0 + dr, f0 + df)).fold(0, |acc, &(dr, df)| acc | bit(r0 + dr, f0 + df))
}

/// Number of KPK positions: white king (64) x black king (64) x pawn on
/// files a-d, ranks 2-7 (24) x side to move (2).
pub const KPK_SIZE: usize = 64 * 64 * 24 * 2;

/// Index layout shared with `src/endgame.rs`.
fn kpk_index(wk: usize, bk: usize, psq: usize, stm: usize) -> usize {
    let p = (psq / 8 - 1) * 4 + psq % 8; // rank 2..7, file a..d
    (((wk * 64 + bk) * 24 + p) * 2) + stm
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum Kpk {
    Invalid,
    Unknown,
    Draw,
    Win,
}

/// Retrograde analysis of K+P vs K (white has the pawn). Returns, for every
/// index, whether white wins with best play.
fn kpk_bitbase(king: &[u64], white_pawn_att: &[u64]) -> Vec<bool> {
    let mut db = vec![Kpk::Invalid; KPK_SIZE];
    let mut coords = Vec::with_capacity(KPK_SIZE);
    for wk in 0..64 {
        for bk in 0..64 {
            for rank in 1..7 {
                for file in 0..4 {
                    let psq = rank * 8 + file;
                    for stm in 0..2 {
                        let idx = kpk_index(wk, bk, psq, stm);
                        coords.push(idx);
                        let push = psq + 8;
                        db[idx] = if wk == bk
                            || wk == psq
                            || bk == psq
                            || king[wk] & (1 << bk) != 0
                            || (stm == 0 && white_pawn_att[psq] & (1 << bk) != 0)
                        {
                            Kpk::Invalid
                        } else if stm == 0
                            && rank == 6
                            && wk != push
                            && bk != push
                            && (king[bk] & (1 << push) == 0 || king[wk] & (1 << push) != 0)
                        {
                            Kpk::Win // safe promotion next move
                        } else if stm == 1
                            && ((king[bk] & !(king[wk] | white_pawn_att[psq])) == 0
                                || king[bk] & (1 << psq) & !king[wk] != 0)
                        {
                            Kpk::Draw // stalemate, or black captures the undefended pawn
                        } else {
                            Kpk::Unknown
                        };
                    }
                }
            }
        }
    }
    let decode = |idx: usize| {
        let stm = idx % 2;
        let p = (idx / 2) % 24;
        let bk = (idx / 48) % 64;
        let wk = idx / (48 * 64);
        (wk, bk, (p / 4 + 1) * 8 + p % 4, stm)
    };
    loop {
        let mut changed = false;
        for &idx in &coords {
            if db[idx] != Kpk::Unknown {
                continue;
            }
            let (wk, bk, psq, stm) = decode(idx);
            let mut seen_win = false;
            let mut seen_draw = false;
            let mut seen_unknown = false;
            let mut visit = |child: usize| match db[child] {
                Kpk::Win => seen_win = true,
                Kpk::Draw => seen_draw = true,
                Kpk::Unknown => seen_unknown = true,
                Kpk::Invalid => {}
            };
            if stm == 0 {
                for to in (0..64).filter(|&s| king[wk] & (1 << s) != 0 && s != psq && king[bk] & (1 << s) == 0) {
                    visit(kpk_index(to, bk, psq, 1));
                }
                let push = psq + 8;
                if psq / 8 < 6 && push != wk && push != bk {
                    visit(kpk_index(wk, bk, push, 1));
                    let push2 = push + 8;
                    if psq / 8 == 1 && push2 != wk && push2 != bk {
                        visit(kpk_index(wk, bk, push2, 1));
                    }
                }
            } else {
                let attacked = king[wk] | white_pawn_att[psq];
                for to in (0..64).filter(|&s| king[bk] & (1 << s) != 0 && attacked & (1 << s) == 0 && s != psq) {
                    visit(kpk_index(wk, to, psq, 0));
                }
            }
            let new = if stm == 0 {
                if seen_win {
                    Kpk::Win
                } else if seen_unknown {
                    Kpk::Unknown
                } else {
                    Kpk::Draw
                }
            } else if seen_draw {
                Kpk::Draw
            } else if seen_unknown {
                Kpk::Unknown
            } else {
                Kpk::Win
            };
            if new != Kpk::Unknown {
                db[idx] = new;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    db.iter().map(|&r| r == Kpk::Win).collect()
}

fn write_array(out: &mut String, name: &str, values: &[u64]) {
    writeln!(out, "pub(crate) static {name}: [u64; {}] = [", values.len()).unwrap();
    for chunk in values.chunks(8) {
        out.push_str("    ");
        for v in chunk {
            write!(out, "{v:#x},").unwrap();
        }
        out.push('\n');
    }
    out.push_str("];\n");
}

fn write_2d(out: &mut String, name: &str, rows: &[Vec<u64>]) {
    writeln!(out, "pub(crate) static {name}: [[u64; {}]; {}] = [", rows[0].len(), rows.len()).unwrap();
    for row in rows {
        out.push_str("    [");
        for v in row {
            write!(out, "{v:#x},").unwrap();
        }
        out.push_str("],\n");
    }
    out.push_str("];\n");
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut out = String::new();
    out.push_str("// @generated by build.rs — do not edit.\n");

    // ---- Magic bitboards (rook entries first, then bishop, in one table) ----
    let mut slider_table: Vec<u64> = Vec::new();
    for (label, dirs) in [("ROOK", &ROOK_DIRS), ("BISHOP", &BISHOP_DIRS)] {
        writeln!(out, "pub(crate) static {label}_MAGICS: [Magic; 64] = [").unwrap();
        for sq in 0..64 {
            let e = find_magic(sq, dirs, &mut rng);
            writeln!(
                out,
                "    Magic {{ mask: {:#x}, magic: {:#x}, offset: {}, shift: {} }},",
                e.mask,
                e.magic,
                slider_table.len(),
                e.shift
            )
            .unwrap();
            slider_table.extend_from_slice(&e.table);
        }
        out.push_str("];\n");
    }
    write_array(&mut out, "SLIDER_ATTACKS", &slider_table);

    // ---- Leapers ----
    let knight: Vec<u64> =
        (0..64).map(|s| leaper(s, &[(1, 2), (2, 1), (2, -1), (1, -2), (-1, -2), (-2, -1), (-2, 1), (-1, 2)])).collect();
    let king: Vec<u64> =
        (0..64).map(|s| leaper(s, &[(1, 0), (1, 1), (0, 1), (-1, 1), (-1, 0), (-1, -1), (0, -1), (1, -1)])).collect();
    let pawn = vec![
        (0..64).map(|s| leaper(s, &[(1, -1), (1, 1)])).collect::<Vec<_>>(),
        (0..64).map(|s| leaper(s, &[(-1, -1), (-1, 1)])).collect::<Vec<_>>(),
    ];
    write_array(&mut out, "KNIGHT_ATTACKS", &knight);
    write_array(&mut out, "KING_ATTACKS", &king);
    write_2d(&mut out, "PAWN_ATTACKS", &pawn);

    // ---- Between / line tables ----
    let mut between = vec![vec![0u64; 64]; 64];
    let mut line = vec![vec![0u64; 64]; 64];
    for a in 0..64 {
        for b in 0..64 {
            if a == b {
                continue;
            }
            for dirs in [&ROOK_DIRS, &BISHOP_DIRS] {
                if slide(a, 0, dirs) & (1u64 << b) != 0 {
                    between[a][b] = slide(a, 1u64 << b, dirs) & slide(b, 1u64 << a, dirs);
                    line[a][b] = (slide(a, 0, dirs) & slide(b, 0, dirs)) | (1u64 << a) | (1u64 << b);
                }
            }
        }
    }
    write_2d(&mut out, "BETWEEN", &between);
    write_2d(&mut out, "LINE", &line);

    // ---- Pawn-structure masks ----
    let mut passed = vec![vec![0u64; 64]; 2];
    for sq in 0..64usize {
        let (r, f) = ((sq / 8) as i32, (sq % 8) as i32);
        for ff in (f - 1)..=(f + 1) {
            if !(0..8).contains(&ff) {
                continue;
            }
            for rr in 0..8 {
                if rr > r {
                    passed[0][sq] |= bit(rr, ff);
                }
                if rr < r {
                    passed[1][sq] |= bit(rr, ff);
                }
            }
        }
    }
    write_2d(&mut out, "PASSED_MASK", &passed);

    // ---- KPK endgame bitbase (white king + pawn vs black king) ----
    let kpk = kpk_bitbase(&king, &pawn[0]);
    let words: Vec<u64> =
        kpk.chunks(64).map(|c| c.iter().enumerate().fold(0u64, |w, (i, &b)| w | (b as u64) << i)).collect();
    write_array(&mut out, "KPK_WIN", &words);

    let dest = PathBuf::from(env::var("OUT_DIR").unwrap()).join("tables.rs");
    fs::write(dest, out).unwrap();
}
