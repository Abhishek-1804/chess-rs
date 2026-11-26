//! Estimated playing strength (Elo) at each search depth.
//!
//! Measured with `examples/elo.rs` (`mise run elo:calibrate`): at each depth
//! the engine (1 thread) played 108 games against Stockfish 19 limited with
//! `UCI_Elo`, whose levels are anchored to the CCRL 40/4 rating list, from 27
//! balanced openings with both colours (Stockfish 500 ms per move at its
//! levels of 1700 and up, 250 ms below). The rating is the maximum-likelihood fit;
//! the standard error is about ±35. CCRL ratings are an engine scale, so treat
//! the numbers as a rough guide to human (FIDE / online) ratings.

/// (depth, Elo, standard error), as measured.
pub const MEASURED: &[(u32, i32, i32)] = &[
    (1, 1301, 33),
    (2, 1451, 34),
    (3, 1551, 33),
    (4, 1660, 33),
    (5, 1797, 34),
    (6, 1985, 34),
    (7, 2235, 35),
    (8, 2410, 34),
    (9, 2529, 33),
    (10, 2580, 34),
    (11, 2706, 34),
    (12, 2761, 34),
];

/// Beyond the measured depths each extra ply is worth this fraction of the
/// previous one's gain: deeper search has diminishing returns.
const TAPER: f64 = 0.8;

/// An Elo estimate for one depth.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Strength {
    pub elo: i32,
    /// From games at this depth (`false` = extrapolated).
    pub measured: bool,
}

/// Estimated Elo when searching `depth` plies.
pub fn estimated_elo(depth: u32) -> Strength {
    let (first, last) = (MEASURED[0], MEASURED[MEASURED.len() - 1]);
    if let Some(&(_, elo, _)) = MEASURED.iter().find(|m| m.0 == depth) {
        return Strength { elo, measured: true };
    }
    if depth < first.0 {
        return Strength { elo: first.1, measured: false };
    }
    // Continue the average gain of the last three measured plies, tapering.
    let n = MEASURED.len().min(4);
    let tail = &MEASURED[MEASURED.len() - n..];
    let mut gain = (tail[n - 1].1 - tail[0].1) as f64 / (n - 1) as f64;
    let mut elo = last.1 as f64;
    for _ in last.0..depth {
        gain *= TAPER;
        elo += gain;
    }
    Strength { elo: elo.round() as i32, measured: false }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measured_depths_are_exact_and_strength_never_drops() {
        for &(d, elo, _) in MEASURED {
            assert_eq!(estimated_elo(d), Strength { elo, measured: true });
        }
        let mut prev = 0;
        for d in 1..=40 {
            let s = estimated_elo(d);
            assert!(s.elo >= prev, "depth {d}");
            prev = s.elo;
        }
        // Diminishing returns keep deep searches within a believable range.
        assert!(estimated_elo(40).elo < 4000);
        assert!(!estimated_elo(MEASURED.last().unwrap().0 + 1).measured);
    }
}
