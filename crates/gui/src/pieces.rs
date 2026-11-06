//! Original vector piece set, rendered to textures by egui_extras' resvg loader.
//!
//! Each piece is one SVG template in a 45x45 view box; colours are substituted
//! per side. resvg rasterises them at the exact on-screen size, so pieces stay
//! crisp at any board size or DPI.

use chess_engine::{Color, Piece, PieceType};

const BASE: &str = r#"<rect x="9" y="35.5" width="27" height="4.5" rx="2"/>"#;

const PAWN: &str = r#"<rect x="11" y="35.5" width="23" height="4.5" rx="2"/>
<path d="M15 35.5 C15 29.5 18.5 26.5 20 25 L25 25 C26.5 26.5 30 29.5 30 35.5 Z"/>
<rect x="16.5" y="21.5" width="12" height="3.5" rx="1.5"/>
<circle cx="22.5" cy="15" r="6"/>"#;

const ROOK: &str = r#"BASE
<rect x="12" y="31" width="21" height="4.5" rx="1"/>
<path d="M14.5 31 L15.5 17 L29.5 17 L30.5 31 Z"/>
<rect x="12" y="13" width="21" height="4" rx="1"/>
<path d="M12 13 V7 H16.5 V10 H20.25 V7 H24.75 V10 H28.5 V7 H33 V13 Z"/>
<path d="M15.2 21 H29.8 M15 27 H30" fill="none" stroke="DETAIL" stroke-width="1"/>"#;

const KNIGHT: &str = r#"BASE
<path d="M31.5 35.5 C33 26 32 15 25 10 L24 5 L20.5 8.8 C15.5 10 11.5 14 10 18.5 L8 23 C7.5 25.2 9.3 27.3 11.5 26.6 L15.5 24.6 C17.5 24 19.2 23 20.6 21.4 C20.4 26.5 15 29 14.2 35.5 Z"/>
<circle cx="16.6" cy="15.6" r="1.4" fill="DETAIL" stroke="none"/>
<circle cx="10.6" cy="23.2" r="0.8" fill="DETAIL" stroke="none"/>
<path d="M26.8 12.5 C29.5 18 30.2 26 29.2 33" fill="none" stroke="DETAIL" stroke-width="1"/>"#;

const BISHOP: &str = r#"BASE
<rect x="13.5" y="31" width="18" height="4.5" rx="1.5"/>
<path d="M22.5 9 C16 13.5 13.5 19.5 15.5 24.5 C16.5 27.5 19 29 19 31 L26 31 C26 29 28.5 27.5 29.5 24.5 C31.5 19.5 29 13.5 22.5 9 Z"/>
<circle cx="22.5" cy="6.5" r="2.6"/>
<path d="M25.5 15 L20.5 21.5 M18.5 27.5 H26.5" fill="none" stroke="DETAIL" stroke-width="1.2"/>"#;

const QUEEN: &str = r#"BASE
<path d="M11.5 33 L8.5 14 L14 25 L15.5 11 L19.6 24 L22.5 9.5 L25.4 24 L29.5 11 L31 25 L36.5 14 L33.5 33 Z"/>
<rect x="11" y="30.5" width="23" height="5" rx="1.5"/>
<circle cx="8.5" cy="12.5" r="2.2"/>
<circle cx="15.5" cy="9.3" r="2.2"/>
<circle cx="22.5" cy="7.6" r="2.2"/>
<circle cx="29.5" cy="9.3" r="2.2"/>
<circle cx="36.5" cy="12.5" r="2.2"/>
<path d="M13 28 H32" fill="none" stroke="DETAIL" stroke-width="1"/>"#;

const KING: &str = r#"BASE
<path d="M12.5 31 C8.5 25.5 8.8 19.2 13.8 17.8 C17.5 16.8 20.6 19.4 22.5 23 C24.4 19.4 27.5 16.8 31.2 17.8 C36.2 19.2 36.5 25.5 32.5 31 Z"/>
<path d="M19.6 23.5 C19.4 18.8 20.6 15.4 22.5 15.4 C24.4 15.4 25.6 18.8 25.4 23.5 Z"/>
<rect x="11.5" y="30.5" width="22" height="5" rx="1.5"/>
<path d="M21 3.5 H24 V7 H27.3 V10 H24 V15 H21 V10 H17.7 V7 H21 Z"/>
<path d="M14 27.5 H31" fill="none" stroke="DETAIL" stroke-width="1"/>"#;

/// Build the SVG document for `piece`.
pub fn svg(piece: Piece) -> String {
    let (fill, stroke, detail) = match piece.color() {
        Color::White => ("#f7f4ee", "#1f1f1f", "#1f1f1f"),
        Color::Black => ("#2a2a2e", "#0d0d0f", "#d8d4cc"),
    };
    let body = match piece.kind() {
        PieceType::Pawn => PAWN,
        PieceType::Knight => KNIGHT,
        PieceType::Bishop => BISHOP,
        PieceType::Rook => ROOK,
        PieceType::Queen => QUEEN,
        PieceType::King => KING,
    };
    let body = body.replace("BASE", BASE).replace("DETAIL", detail);
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 45 45"><g fill="{fill}" stroke="{stroke}" stroke-width="1.5" stroke-linejoin="round" stroke-linecap="round">{body}</g></svg>"#
    )
}

/// Stable URI for the piece's image (the `.svg` suffix selects the loader).
pub fn uri(piece: Piece) -> String {
    format!("bytes://chess-rs/{}{}.svg", if piece.color() == Color::White { 'w' } else { 'b' }, piece.kind().to_char())
}

/// All twelve images, built once.
pub struct PieceImages {
    images: Vec<egui::Image<'static>>,
}

impl PieceImages {
    pub fn new() -> Self {
        let images = Piece::ALL.iter().map(|&p| egui::Image::from_bytes(uri(p), svg(p).into_bytes())).collect();
        PieceImages { images }
    }

    pub fn get(&self, p: Piece) -> &egui::Image<'static> {
        &self.images[p.idx()]
    }
}

impl Default for PieceImages {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn svgs_are_well_formed_and_distinct() {
        let all: Vec<String> = Piece::ALL.iter().map(|&p| svg(p)).collect();
        for s in &all {
            assert!(s.starts_with("<svg") && s.ends_with("</svg>"));
            assert!(!s.contains("BASE") && !s.contains("DETAIL"));
            assert_eq!(s.matches('<').count(), s.matches('>').count());
        }
        for i in 0..all.len() {
            for j in i + 1..all.len() {
                assert_ne!(all[i], all[j]);
            }
        }
    }
}
