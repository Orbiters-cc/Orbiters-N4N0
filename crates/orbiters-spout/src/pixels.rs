//! What both receivers do with pixels: 16-bit frames to the 8-bit codes every decoder expects, and which rows of a frame were read.

use std::sync::OnceLock;

pub(crate) fn half_to_f32(h: u16) -> f32 {
    let sign = ((h >> 15) & 1) as u32;
    let exp = ((h >> 10) & 0x1F) as u32;
    let frac = (h & 0x3FF) as u32;
    let bits = if exp == 0 {
        if frac == 0 {
            sign << 31
        } else {
            let mut e: i32 = 127 - 15 + 1;
            let mut f = frac;
            while f & 0x400 == 0 {
                f <<= 1;
                e -= 1;
            }
            (sign << 31) | ((e as u32) << 23) | ((f & 0x3FF) << 13)
        }
    } else if exp == 0x1F {
        (sign << 31) | 0x7F80_0000 | (frac << 13)
    } else {
        (sign << 31) | ((exp + 127 - 15) << 23) | (frac << 13)
    };
    f32::from_bits(bits)
}

pub(crate) fn linear_to_srgb8(v: f32) -> u8 {
    let v = v.clamp(0.0, 1.0);
    let s = if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
    (s * 255.0 + 0.5) as u8
}

/// Every half float's 8-bit sRGB code (negative and NaN give 0, above 1 gives 255), built once: a 16-bit sender's
/// frame then costs a table lookup per channel instead of a `powf`.
pub(crate) fn half_to_srgb8_table() -> &'static [u8; 65536] {
    static TABLE: OnceLock<Box<[u8; 65536]>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut table = Box::new([0u8; 65536]);
        for (h, code) in table.iter_mut().enumerate() {
            *code = linear_to_srgb8(half_to_f32(h as u16));
        }
        table
    })
}

/// Adds `range` to a sorted list of disjoint row ranges, merging those that touch.
pub(crate) fn add_rows(list: &mut Vec<(u32, u32)>, range: (u32, u32)) {
    list.push(range);
    list.sort_unstable();
    let mut merged: Vec<(u32, u32)> = Vec::with_capacity(list.len());
    for &(a, b) in list.iter() {
        match merged.last_mut() {
            Some(last) if a <= last.1 => last.1 = last.1.max(b),
            _ => merged.push((a, b)),
        }
    }
    *list = merged;
}

/// The parts of `range` a sorted list of disjoint ranges does not cover.
pub(crate) fn missing_rows(range: (u32, u32), list: &[(u32, u32)]) -> Vec<(u32, u32)> {
    let mut out = Vec::new();
    let mut from = range.0;
    for &(a, b) in list {
        if b <= from {
            continue;
        }
        if a >= range.1 {
            break;
        }
        if a > from {
            out.push((from, a));
        }
        from = from.max(b);
        if from >= range.1 {
            break;
        }
    }
    if from < range.1 {
        out.push((from, range.1));
    }
    out
}
