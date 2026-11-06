//! Zobrist keys, generated at compile time with a fixed seed.
//!
//! The keys are deterministic across builds and runs, which is what makes the
//! on-disk position cache valid from one session to the next.

pub struct Zobrist {
    pub pieces: [[u64; 64]; 12],
    pub castling: [u64; 16],
    pub ep: [u64; 8],
    pub side: u64,
}

const fn splitmix(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

const fn generate() -> Zobrist {
    let mut s = 0x00C0_FFEE_D00D_F00Du64;
    let mut pieces = [[0u64; 64]; 12];
    let mut p = 0;
    while p < 12 {
        let mut sq = 0;
        while sq < 64 {
            pieces[p][sq] = splitmix(&mut s);
            sq += 1;
        }
        p += 1;
    }
    let mut castling = [0u64; 16];
    // Keys for individual rights; combined rights XOR them so updates compose.
    let base = [splitmix(&mut s), splitmix(&mut s), splitmix(&mut s), splitmix(&mut s)];
    let mut c = 0;
    while c < 16 {
        let mut k = 0;
        let mut b = 0;
        while b < 4 {
            if c & (1 << b) != 0 {
                k ^= base[b];
            }
            b += 1;
        }
        castling[c] = k;
        c += 1;
    }
    let mut ep = [0u64; 8];
    let mut f = 0;
    while f < 8 {
        ep[f] = splitmix(&mut s);
        f += 1;
    }
    Zobrist { pieces, castling, ep, side: splitmix(&mut s) }
}

pub static ZOBRIST: Zobrist = generate();
