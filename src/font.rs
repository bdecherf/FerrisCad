//! A small stroke (single-line) font for text on the board: what you see on screen is
//! exactly what goes to the silkscreen or copper Gerbers.
//!
//! Glyphs are polylines on a 4 × 6 grid (y down, 0 = top of capitals). Lower-case letters
//! have their own glyphs; accented letters lose their accent.

use crate::model::{rot_f, BoardText, TextLayer};

fn glyph(c: char) -> Option<&'static str> {
    Some(match c {
        'A' => "0,6 0,2 2,0 4,2 4,6|0,4 4,4",
        'B' => "0,0 3,0 4,1 4,2 3,3 0,3|3,3 4,4 4,5 3,6 0,6 0,0",
        'C' => "4,1 3,0 1,0 0,1 0,5 1,6 3,6 4,5",
        'D' => "0,0 2,0 4,2 4,4 2,6 0,6 0,0",
        'E' => "4,0 0,0 0,6 4,6|0,3 3,3",
        'F' => "4,0 0,0 0,6|0,3 3,3",
        'G' => "4,1 3,0 1,0 0,1 0,5 1,6 3,6 4,5 4,3 2,3",
        'H' => "0,0 0,6|4,0 4,6|0,3 4,3",
        'I' => "1,0 3,0|2,0 2,6|1,6 3,6",
        'J' => "4,0 4,5 3,6 1,6 0,5",
        'K' => "0,0 0,6|4,0 0,4|1,3 4,6",
        'L' => "0,0 0,6 4,6",
        'M' => "0,6 0,0 2,3 4,0 4,6",
        'N' => "0,6 0,0 4,6 4,0",
        'O' => "1,0 3,0 4,1 4,5 3,6 1,6 0,5 0,1 1,0",
        'P' => "0,6 0,0 3,0 4,1 4,2 3,3 0,3",
        'Q' => "1,0 3,0 4,1 4,5 3,6 1,6 0,5 0,1 1,0|2,4 4,6",
        'R' => "0,6 0,0 3,0 4,1 4,2 3,3 0,3|2,3 4,6",
        'S' => "4,1 3,0 1,0 0,1 0,2 1,3 3,3 4,4 4,5 3,6 1,6 0,5",
        'T' => "0,0 4,0|2,0 2,6",
        'U' => "0,0 0,5 1,6 3,6 4,5 4,0",
        'V' => "0,0 2,6 4,0",
        'W' => "0,0 1,6 2,3 3,6 4,0",
        'X' => "0,0 4,6|4,0 0,6",
        'Y' => "0,0 2,3 4,0|2,3 2,6",
        'Z' => "0,0 4,0 0,6 4,6",
        '0' => "1,0 3,0 4,1 4,5 3,6 1,6 0,5 0,1 1,0|4,1 0,5",
        '1' => "1,1 2,0 2,6|1,6 3,6",
        '2' => "0,1 1,0 3,0 4,1 4,2 0,6 4,6",
        '3' => "0,1 1,0 3,0 4,1 4,2 3,3 1,3|3,3 4,4 4,5 3,6 1,6 0,5",
        '4' => "3,6 3,0 0,4 4,4",
        '5' => "4,0 0,0 0,3 3,3 4,4 4,5 3,6 0,6",
        '6' => "3,0 1,0 0,1 0,5 1,6 3,6 4,5 4,4 3,3 0,3",
        '7' => "0,0 4,0 1,6",
        '8' => "1,0 3,0 4,1 4,2 3,3 1,3 0,2 0,1 1,0|1,3 0,4 0,5 1,6 3,6 4,5 4,4 3,3",
        '9' => "4,3 1,3 0,2 0,1 1,0 3,0 4,1 4,5 3,6 1,6",
        '-' => "1,3 3,3",
        '_' => "0,6 4,6",
        '.' => "1.8,6 2.2,6",
        ',' => "2,5.2 1.4,7",
        ':' => "1.8,2 2.2,2|1.8,6 2.2,6",
        ';' => "1.8,2 2.2,2|2,5.2 1.4,7",
        '/' => "0,6 4,0",
        '\\' => "0,0 4,6",
        '(' => "3,0 2,1 2,5 3,6",
        ')' => "1,0 2,1 2,5 1,6",
        '[' => "3,0 2,0 2,6 3,6",
        ']' => "1,0 2,0 2,6 1,6",
        '+' => "2,1 2,5|0,3 4,3",
        '=' => "0,2 4,2|0,4 4,4",
        '!' => "2,0 2,4|1.8,6 2.2,6",
        '?' => "0,1 1,0 3,0 4,1 4,2 2,3 2,4|1.8,6 2.2,6",
        '\'' => "2,0 2,1.5",
        '"' => "1,0 1,1.5|3,0 3,1.5",
        '#' => "1,0 1,6|3,0 3,6|0,2 4,2|0,4 4,4",
        '%' => "0,6 4,0|0,0 1,0 1,1 0,1 0,0|3,5 4,5 4,6 3,6 3,5",
        '&' => "4,6 1,1 2,0 3,1 0,4 0,5 1,6 2,6 4,4",
        '<' => "4,0 0,3 4,6",
        '>' => "0,0 4,3 0,6",
        '*' => "2,1 2,5|0,2 4,4|0,4 4,2",
        '°' => "1,0 2,0 2,1 1,1 1,0",
        'Ω' => "0,6 1,6 1,5 0,3 0,1 1,0 3,0 4,1 4,3 3,5 3,6 4,6",
        'µ' | 'μ' => "0,7 0,2|0,5 1,6 3,6 4,5|4,2 4,6",
        // Lower case: x-height from 2 to 6, descenders down to 8.
        'a' => "4,2 4,6|4,3 3,2 1,2 0,3 0,5 1,6 3,6 4,5",
        'b' => "0,0 0,6|0,3 1,2 3,2 4,3 4,5 3,6 1,6 0,5",
        'c' => "4,3 3,2 1,2 0,3 0,5 1,6 3,6 4,5",
        'd' => "4,0 4,6|4,3 3,2 1,2 0,3 0,5 1,6 3,6 4,5",
        'e' => "0,4 4,4 4,3 3,2 1,2 0,3 0,5 1,6 3,6 4,5.5",
        'f' => "3.5,0 2.5,0 1.5,1 1.5,6|0,2 3,2",
        'g' => "4,2 4,7 3,8 1,8|4,3 3,2 1,2 0,3 0,5 1,6 3,6 4,5",
        'h' => "0,0 0,6|0,3 1,2 3,2 4,3 4,6",
        'i' => "2,2 2,6|1.8,0.6 2.2,0.6",
        'j' => "3,2 3,7 2,8 1,8|2.8,0.6 3.2,0.6",
        'k' => "0,0 0,6|4,2 0,5|1.5,4 4,6",
        'l' => "2,0 2,5 3,6",
        'm' => "0,6 0,2|0,3 1,2 2,3 2,6|2,3 3,2 4,3 4,6",
        'n' => "0,6 0,2|0,3 1,2 3,2 4,3 4,6",
        'o' => "1,2 3,2 4,3 4,5 3,6 1,6 0,5 0,3 1,2",
        'p' => "0,2 0,8|0,3 1,2 3,2 4,3 4,5 3,6 1,6 0,5",
        'q' => "4,2 4,8|4,3 3,2 1,2 0,3 0,5 1,6 3,6 4,5",
        'r' => "0,2 0,6|0,4 2,2 4,2",
        's' => "4,2 1,2 0,3 1,4 3,4 4,5 3,6 0,6",
        't' => "1.5,0 1.5,5 2.5,6 3.5,6|0,2 3,2",
        'u' => "0,2 0,5 1,6 3,6 4,5|4,2 4,6",
        'v' => "0,2 2,6 4,2",
        'w' => "0,2 1,6 2,3 3,6 4,2",
        'x' => "0,2 4,6|4,2 0,6",
        'y' => "0,2 2,6|4,2 1,8",
        'z' => "0,2 4,2 0,6 4,6",
        _ => return None,
    })
}

fn plain(c: char) -> char {
    match c {
        'à' | 'â' | 'ä' | 'á' => 'a',
        'é' | 'è' | 'ê' | 'ë' => 'e',
        'î' | 'ï' | 'í' => 'i',
        'ô' | 'ö' | 'ó' => 'o',
        'ù' | 'û' | 'ü' | 'ú' => 'u',
        'ç' => 'c',
        'À' | 'Â' => 'A',
        'É' | 'È' | 'Ê' => 'E',
        'Ç' => 'C',
        _ => c,
    }
}

/// Polylines of a text whose capitals are `size` tall, centred on (0, 0), y down.
pub fn layout(text: &str, size: f32) -> Vec<Vec<(f32, f32)>> {
    let u = size / 6.0;
    let mut out = vec![];
    let mut x = 0.0f32;
    for ch in text.chars() {
        let ch = plain(ch);
        if ch == ' ' {
            x += 3.5;
            continue;
        }
        // Real lower-case glyph when there is one, else a small capital on the baseline.
        let lower = ch.is_lowercase() && glyph(ch).is_none();
        let up = if lower { ch.to_uppercase().next().unwrap_or(ch) } else { ch };
        let (sx, sy, dy) = if lower { (0.8, 0.7, 1.8) } else { (1.0, 1.0, 0.0) };
        match glyph(up) {
            Some(g) => {
                // Proportional spacing: narrow letters (i, l, 1, punctuation) take less room.
                let coords: Vec<(f32, f32)> = g
                    .split(['|', ' '])
                    .filter_map(|p| {
                        let (a, b) = p.split_once(',')?;
                        Some((a.parse::<f32>().ok()?, b.parse::<f32>().ok()?))
                    })
                    .collect();
                let (gx0, gx1) = coords.iter().fold((f32::MAX, f32::MIN), |m, c| (m.0.min(c.0), m.1.max(c.0)));
                let shift = -gx0 * sx;
                for stroke in g.split('|') {
                    let pts: Vec<(f32, f32)> = stroke
                        .split_whitespace()
                        .filter_map(|p| {
                            let (a, b) = p.split_once(',')?;
                            Some((x + shift + a.parse::<f32>().ok()? * sx, dy + b.parse::<f32>().ok()? * sy))
                        })
                        .collect();
                    if pts.len() >= 2 {
                        out.push(pts);
                    }
                }
                x += ((gx1 - gx0) * sx).max(0.4) + 1.2;
            }
            None => {
                // Unknown character: a small box.
                out.push(vec![(x, 1.0), (x + 4.0, 1.0), (x + 4.0, 6.0), (x, 6.0), (x, 1.0)]);
                x += 5.0;
            }
        }
    }
    let width = (x - 1.2).max(0.0);
    out.into_iter()
        .map(|s| s.into_iter().map(|(px, py)| ((px - width / 2.0) * u, (py - 3.0) * u)).collect())
        .collect()
}

/// Line width used for a text of this size.
pub fn stroke_width(size: f32) -> f32 {
    (size * 0.15).max(0.1)
}

/// The text's strokes in board coordinates (rotated, and mirrored on the back side).
pub fn board_strokes(t: &BoardText) -> Vec<Vec<(f32, f32)>> {
    let mirror = t.layer.is_back();
    layout(&t.text, t.size)
        .into_iter()
        .map(|s| {
            s.into_iter()
                .map(|(x, y)| {
                    let x = if mirror { -x } else { x };
                    let r = rot_f((x, y), t.rot);
                    (t.pos.0 + r.0, t.pos.1 + r.1)
                })
                .collect()
        })
        .collect()
}

/// Bounding box of a board text.
pub fn bbox(t: &BoardText) -> (f32, f32, f32, f32) {
    let mut b = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for s in board_strokes(t) {
        for (x, y) in s {
            b = (b.0.min(x), b.1.min(y), b.2.max(x), b.3.max(y));
        }
    }
    if b.0 > b.2 {
        (t.pos.0, t.pos.1, t.pos.0, t.pos.1)
    } else {
        let w = stroke_width(t.size) / 2.0;
        (b.0 - w, b.1 - w, b.2 + w, b.3 + w)
    }
}

impl TextLayer {
    pub fn is_back(self) -> bool {
        matches!(self, TextLayer::SilkBack | TextLayer::Copper(crate::model::Layer::Back))
    }
}
