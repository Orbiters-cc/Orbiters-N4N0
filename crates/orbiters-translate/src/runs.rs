//! A line as the detector finds it can run over more than text: on a station sign "← 西口 ← [toilets] 北口 ↑" was one
//! box (sandbox, 2026-11-13), its cover hid the pictogram and its translation was drawn as tall as the whole box. Here a
//! line is cut into the runs of letters it really holds, from where the recogniser read each letter and how sure it was:
//! arrows, symbols and what it was unsure of (a pictogram read as a stray letter) part runs and are left visible, and so
//! do gaps wider than a letter. Each run gets a box tight to its letters (their height, a little more), so that its
//! cover hides only text and its translation is never taller than the original letters.

use crate::ocr::{Line, TextBox};

/// A letter counts as text from this sureness.
const SURE_LETTER: f32 = 0.5;
/// Letters' middles further apart than this many letter heights part two runs (a Latin word space is about one, a Korean one two; a pictogram between two labels several).
const GAP: f32 = 3.0;
/// A run's box: its letters' height times this, and this much of a letter height before and after them.
pub const RUN_HEIGHT: f32 = 1.5;
const RUN_PAD: f32 = 0.45;

/// The letters' own height in a box the detector made larger by its "unclip" distance on every side (DB's
/// `d = area × 1.5 / perimeter`, found back by halving: the larger the letters, the larger the box).
pub fn letter_height(b: &TextBox) -> f32 {
    let (big_w, big_h) = (b.length(), b.thickness());
    // The box made of letters `h` high (and as much shorter along as across).
    let grown = |h: f32| {
        let w = (big_w - big_h + h).max(h);
        h + 2.0 * 1.5 * w * h / (2.0 * (w + h)).max(1e-3)
    };
    let (mut lo, mut hi) = (0.0f32, big_h);
    for _ in 0..40 {
        let mid = (lo + hi) / 2.0;
        if grown(mid) > big_h {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    lo.max(1.0)
}

fn letters_in(s: &str) -> bool {
    s.chars().any(char::is_alphanumeric)
}

/// Marks that belong inside a run of text (between its letters), but never start or end one.
fn joins(s: &str) -> bool {
    let t = s.trim();
    t.is_empty() || t.chars().all(|c| "、。，．・ー！？!?.,'’:;-&()「」『』（）【】/".contains(c))
}

/// A point of the box at (along, across), both 0..1.
fn at(b: &TextBox, along: f32, across: f32) -> [f32; 2] {
    let [tl, tr, br, bl] = b.corners;
    let top = [tl[0] + (tr[0] - tl[0]) * along, tl[1] + (tr[1] - tl[1]) * along];
    let bottom = [bl[0] + (br[0] - bl[0]) * along, bl[1] + (br[1] - bl[1]) * along];
    [top[0] + (bottom[0] - top[0]) * across, top[1] + (bottom[1] - top[1]) * across]
}

/// The runs of text of a line, each a line of its own with a box tight to its letters. A line read without letter
/// places (another reader's) is kept whole.
pub fn split(line: &Line) -> Vec<Line> {
    if line.glyphs.is_empty() {
        return vec![line.clone()];
    }
    let (length, thickness) = (line.boxed.length().max(1.0), line.boxed.thickness().max(1.0));
    let h = letter_height(&line.boxed);
    // CTC places a letter as a narrow peak, not over its width: gaps are measured between letters' middles. Letters of
    // one word or one CJK label are about a letter height apart or less; a pictogram or a panel's edge is several.
    let gap = GAP * h / length;
    let middle = |i: usize| (line.glyphs[i].1[0] + line.glyphs[i].1[1]) / 2.0;
    // Runs of glyph indices.
    let mut runs: Vec<Vec<usize>> = Vec::new();
    let mut current: Vec<usize> = Vec::new();
    let mut last_letter: Option<usize> = None;
    for (i, (text, _, _)) in line.glyphs.iter().enumerate() {
        // Any letter, sure or not: a misread letter inside a word ("West Fxit") does not part it. Unsure letters are only
        // left out at a run's ends (below).
        if letters_in(text) {
            if let Some(j) = last_letter {
                if middle(i) - middle(j) > gap {
                    runs.push(std::mem::take(&mut current));
                }
            }
            current.push(i);
            last_letter = Some(i);
        } else if joins(text) && !current.is_empty() {
            current.push(i);
        } else {
            // An arrow, a symbol, a letter read without sureness: the run ends before it.
            runs.push(std::mem::take(&mut current));
            last_letter = None;
        }
    }
    runs.push(current);
    let half = (RUN_HEIGHT * h / 2.0 / thickness).min(0.5);
    let pad = RUN_PAD * h / length;
    runs.into_iter()
        .filter_map(|run| {
            // Marks, and letters read without sureness (a pictogram's edge read as one), at its ends are not part of it.
            let kept = |i: &&usize| letters_in(&line.glyphs[**i].0) && line.glyphs[**i].2 >= SURE_LETTER;
            let first = *run.iter().find(kept)?;
            let last = *run.iter().rev().find(kept)?;
            let text: String = line.glyphs[first..=last].iter().map(|g| g.0.as_str()).collect::<String>().trim().to_string();
            let sure: Vec<f32> = line.glyphs[first..=last].iter().filter(|g| letters_in(&g.0)).map(|g| g.2).collect();
            let (a0, a1) = ((line.glyphs[first].1[0] - pad).max(0.0), (line.glyphs[last].1[1] + pad).min(1.0));
            let (c0, c1) = (0.5 - half, 0.5 + half);
            let corners = [at(&line.boxed, a0, c0), at(&line.boxed, a1, c0), at(&line.boxed, a1, c1), at(&line.boxed, a0, c1)];
            let glyphs = line.glyphs[first..=last].iter().map(|(t, s, p)| (t.clone(), [((s[0] - a0) / (a1 - a0).max(1e-4)).clamp(0.0, 1.0), ((s[1] - a0) / (a1 - a0).max(1e-4)).clamp(0.0, 1.0)], *p)).collect();
            Some(Line {
                boxed: TextBox { corners, score: line.boxed.score, vertical: line.boxed.vertical },
                text,
                confidence: sure.iter().sum::<f32>() / sure.len().max(1) as f32,
                reader: line.reader,
                glyphs,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ocr::Reader;

    fn line(glyphs: &[(&str, f32, f32, f32)]) -> Line {
        // A box 1000 x 100 px (unclipped), horizontal.
        let boxed = TextBox { corners: [[0.0, 0.0], [1000.0, 0.0], [1000.0, 100.0], [0.0, 100.0]], score: 0.9, vertical: false };
        Line { boxed, text: glyphs.iter().map(|g| g.0).collect(), confidence: 0.9, reader: Reader::Main, glyphs: glyphs.iter().map(|g| (g.0.to_string(), [g.1, g.2], g.3)).collect() }
    }

    #[test]
    fn the_letters_height_is_found_inside_the_unclipped_box() {
        // Letters 40 px high on a 900 px line: d = 1.5 * 900 * 40 / (2 * 940) = 28.7, so 957 x 97 unclipped.
        let b = TextBox { corners: [[0.0, 0.0], [957.4, 0.0], [957.4, 97.4], [0.0, 97.4]], score: 1.0, vertical: false };
        assert!((letter_height(&b) - 40.0).abs() < 1.0, "{}", letter_height(&b));
    }

    #[test]
    fn arrows_and_pictograms_part_a_line_into_its_runs_of_text() {
        // "← 西口 ← [pictogram read as an unsure 'S'] 北口 ↑", as on the station's sign.
        let l = line(&[("←", 0.02, 0.06, 0.9), ("西", 0.10, 0.14, 0.99), ("口", 0.14, 0.18, 0.99), ("←", 0.30, 0.34, 0.8), ("S", 0.42, 0.45, 0.31), ("北", 0.62, 0.66, 0.98), ("口", 0.66, 0.70, 0.99), ("↑", 0.85, 0.9, 0.7)]);
        let runs = split(&l);
        assert_eq!(runs.iter().map(|r| r.text.as_str()).collect::<Vec<_>>(), vec!["西口", "北口"]);
        // The first run's box ends before the arrow after it, and nothing covers the pictogram.
        let b = runs[0].boxed.bounds();
        assert!(b[0] > 60.0 && b[2] < 300.0, "{b:?}");
        assert!(!runs.iter().any(|r| r.boxed.bounds()[0] < 450.0 && r.boxed.bounds()[2] > 420.0), "the pictogram stays visible");
        // As tall as the letters, a little more: never the whole box.
        let h = letter_height(&l.boxed);
        assert!((runs[0].boxed.thickness() - RUN_HEIGHT * h).abs() < 1.0 && runs[0].boxed.thickness() < l.boxed.thickness());
    }

    #[test]
    fn words_stay_together_and_far_ones_part() {
        let l = line(&[("W", 0.10, 0.12, 0.99), ("e", 0.12, 0.14, 0.99), (" ", 0.14, 0.16, 0.9), ("s", 0.16, 0.18, 0.99), ("t", 0.18, 0.2, 0.99), ("E", 0.70, 0.72, 0.99), ("x", 0.72, 0.74, 0.99)]);
        let runs = split(&l);
        assert_eq!(runs.iter().map(|r| r.text.as_str()).collect::<Vec<_>>(), vec!["We st", "Ex"]);
        assert_eq!(split(&Line { glyphs: vec![], ..l.clone() }).len(), 1, "a line without letter places is kept whole");
    }
}
