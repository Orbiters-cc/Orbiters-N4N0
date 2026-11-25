//! The layer behind a capture standing small in a panel's hole: our own backdrop, so that the hole reads as a window onto a small place and not
//! as a hole onto VRChat's menu and the world (Enzo, 2026-11-12, G1).
//!
//! Why it is drawn into the capture's own pictures and is no overlay of its own: the capture is the cube's faces, 1.5 m from the eyes
//! ([`super::view::HALF_M`]), and SteamVR stacked the overlays here by distance whatever their sort orders said (F7, IMG2): a pane in the
//! page's plane, nearer than the faces, covered the capture; a pane behind them would have to be farther than 1.5 m, and would then show
//! past the hole's edges to one eye or the other as the head moves. Drawn into the faces' texels behind the splats' (premultiplied, the
//! splats over it), it can never come in front of the capture, and it is clipped per eye and per texel to the rays that go through the
//! hole (grown by [`GROW_M`] under the sheet's opaque edge, never past the sheet), so each eye sees it fill the whole opening from any
//! angle, with no gap at the frame when a hand moves the panel between two pictures. Where the rays meet it, it lies on a plane
//! [`DEPTH_PER_RADIUS`] behind the hole, behind all of the capture: both eyes see it at that depth, behind what stands in the hole.
//!
//! Its look: a deep ink a little cooler than the sheet, lighter towards a low horizon, a faint pool of the accent under the capture's middle
//! (the ground it stands on), and a soft shade inside the frame's edge (the panel's thickness). The retro look has one flat ink, as its
//! grounds have. The GPU draws it in `resample.wgsl`; [`Opening::colour`] here is the same, for the tests and the screenshots.

use super::stereo::{add, dot, length, position, scale, sub, Cube, Face, FACES, V3};
use orbiters_openvr::Matrix34;

/// How far past the hole the backdrop reaches, under the sheet's opaque edge: a hand moves the panel between two pictures (one batch,
/// 33 ms at 30 a second, a little more under load) without the edge of what was drawn showing in the opening.
pub const GROW_M: f32 = 0.018;
/// How far inside the sheet's own edge it stops: SteamVR's filtering of the edge, a texel or two. Where the hole's right side leaves 15 mm of
/// sheet, every millimetre counts (a 7.5 mm move seen from 40 degrees aside showed a sliver with 6 mm here).
pub const INSET_M: f32 = 0.002;
/// How far from its corners the sheet's edge is straight: its rounded corners (16 units, 14.4 mm at size 1) and the retro look's cut ones
/// (14 units at 45 degrees), with room for the larger sizes.
pub const SHEET_CORNER_M: f32 = 0.022;
/// How far behind the hole the backdrop lies, in the capture's radius there: its round lies one radius behind the hole and reaches one
/// more ([`super::stereo::SmallDock::behind`]).
pub const DEPTH_PER_RADIUS: f32 = 2.6;
/// How far a panel is held, for the backdrop's light to sit in its hole as it was designed whatever the distance.
const HELD_AT_M: f32 = 0.5;
/// How much of the frame's edge is shaded inside it, and how dark it gets at the edge.
const SHADE_M: f32 = 0.035;
const SHADE_AT_EDGE: f32 = 0.62;
/// A move of the panel under this, or a turn under this many radians, is its trembling in a hand: the pictures are not drawn again for it.
const TREMBLE_M: f32 = 0.003;
const TREMBLE_TURN: f32 = 0.01;

/// The backdrop's colours (sRGB-encoded, 0 to 1, as the faces' textures hold them).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    /// High in the opening, and under the far ground.
    pub top: [f32; 3],
    /// At the horizon, a little under the opening's middle.
    pub horizon: [f32; 3],
    /// The pool of light under the capture's middle, added.
    pub glow: [f32; 3],
    /// One colour, no light and no shade (the retro look).
    pub flat: bool,
}

impl Look {
    /// Ours, with the accent chosen in Settings: Orbiters' ink, from under `theme::ink_950()` to a cooler ink under `INK_800`, darker than the
    /// side panel around it (`INK_800`) so the frame reads as the near edge and the capture is lit against it.
    pub fn ours(accent: [u8; 3], retro: bool) -> Self {
        let c = |v: u8| v as f32 / 255.0;
        Self {
            top: [c(0x08), c(0x09), c(0x0c)],
            // Light enough at the horizon that where the capture is sparse the opening reads as distance, not as a black void (the
            // design critique of the first renders), still under the panel's ink.
            horizon: [c(0x18), c(0x1a), c(0x20)],
            glow: [c(accent[0]) * 0.03, c(accent[1]) * 0.03, c(accent[2]) * 0.03],
            flat: retro,
        }
    }
}

/// The opening a capture standing small is seen through, as the pictures of the cube need it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Opening {
    /// The hole's middle, its X across, Y up and Z towards who looks (the sheet's plane).
    pub place: Matrix34,
    /// Half the hole's width and height, in metres.
    pub half: [f32; 2],
    /// Where the backdrop is drawn, from the hole's middle in its plane (min x, min y, max x, max y): the hole grown under the sheet.
    pub cover: [f32; 4],
    /// How far behind the hole it lies.
    pub depth: f32,
    pub look: Look,
}

/// `x` from `a` (0) to `b` (1), eased; `a` may be over `b`.
fn ease(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

/// Under a half step of 8 bits, by texel: no bands in the dark gradient. As `dither` in `resample.wgsl`.
pub fn dither(x: u32, y: u32) -> f32 {
    let mut h = x.wrapping_mul(1973).wrapping_add(y.wrapping_mul(9277)).wrapping_add(89);
    h = (h ^ (h >> 13)).wrapping_mul(1_274_126_177);
    h ^= h >> 16;
    ((h & 255) as f32 / 255.0 - 0.5) / 255.0
}

impl Opening {
    /// The opening of a hole whose middle is `place`, `half` its half size, in a sheet whose opaque picture covers `sheet` (from the hole's
    /// middle, in its plane: min x, min y, max x, max y), for a capture reaching `radius` there.
    pub fn in_sheet(place: Matrix34, half: [f32; 2], sheet: [f32; 4], radius: f32, look: Look) -> Self {
        let lo = |h: f32, edge: f32| (-h - GROW_M).max(edge + INSET_M).min(-h);
        let hi = |h: f32, edge: f32| (h + GROW_M).min(edge - INSET_M).max(h);
        let mut cover = [lo(half[0], sheet[0]), lo(half[1], sheet[1]), hi(half[0], sheet[2]), hi(half[1], sheet[3])];
        // Its corners inside the sheet's own: where it comes as near a side as the sheet's corners are round (or cut), it keeps that far from
        // the sheet's top and foot, as long as the hole allows.
        if cover[0] < sheet[0] + SHEET_CORNER_M || cover[2] > sheet[2] - SHEET_CORNER_M {
            cover[1] = cover[1].max(sheet[1] + SHEET_CORNER_M).min(-half[1]);
            cover[3] = cover[3].min(sheet[3] - SHEET_CORNER_M).max(half[1]);
        }
        Self { place, half, cover, depth: radius * DEPTH_PER_RADIUS, look }
    }

    fn axis(&self, i: usize) -> V3 {
        [self.place.m[0][i], self.place.m[1][i], self.place.m[2][i]]
    }

    /// Where a ray from `eye` along `dir` crosses the hole's plane, from its middle (across, up), when that is inside what the backdrop
    /// covers; `None` past it, from behind the sheet, or for a ray that does not go into the sheet.
    pub fn hit(&self, eye: V3, dir: V3) -> Option<[f32; 2]> {
        let ez = self.axis(2);
        let dz = dot(dir, ez);
        let front = dot(sub(position(&self.place), eye), ez);
        if dz >= -1e-6 || front >= 0.0 {
            return None;
        }
        let p = sub(add(eye, scale(dir, front / dz)), position(&self.place));
        let (u, v) = (dot(p, self.axis(0)), dot(p, self.axis(1)));
        let c = self.cover;
        (u >= c[0] && u <= c[2] && v >= c[1] && v <= c[3]).then_some([u, v])
    }

    /// How much wider the back plane's part seen through the hole is than the hole, from where a panel is held.
    fn spread(&self) -> f32 {
        1.0 + self.depth / HELD_AT_M
    }

    /// The backdrop's colour seen from `eye` along `dir` at texel `texel` (for its dither), where it covers.
    pub fn colour(&self, eye: V3, dir: V3, texel: [u32; 2]) -> Option<[f32; 3]> {
        let [u, v] = self.hit(eye, dir)?;
        let look = &self.look;
        if look.flat {
            return Some(scale(look.horizon, 0.85));
        }
        let ez = self.axis(2);
        let front = dot(sub(position(&self.place), eye), ez);
        let q = sub(add(eye, scale(dir, (front - self.depth) / dot(dir, ez))), position(&self.place));
        let k = self.spread();
        let n = [dot(q, self.axis(0)) / (self.half[0] * k), dot(q, self.axis(1)) / (self.half[1] * k)];
        let mut base = mix(look.horizon, look.top, ease(-0.35, 1.0, n[1]));
        base = mix(base, look.top, 0.5 * ease(-0.35, -1.3, n[1]));
        let pool = (-(n[0] * n[0] / 0.8 + (n[1] + 0.55) * (n[1] + 0.55) / 0.08)).exp();
        base = add(base, scale(look.glow, pool));
        let edge = (self.half[0] - u.abs()).min(self.half[1] - v.abs());
        let shade = SHADE_AT_EDGE + (1.0 - SHADE_AT_EDGE) * ease(0.0, SHADE_M, edge);
        let d = dither(texel[0], texel[1]);
        Some(base.map(|c| c * shade + d))
    }

    /// Whether the panel moved or turned since `drawn` was drawn, more than a hand trembles, or its hole or look changed.
    pub fn moved_from(&self, drawn: &Opening) -> bool {
        let turned = (0..3).any(|i| dot(self.axis(i), drawn.axis(i)) < TREMBLE_TURN.cos());
        length(sub(position(&self.place), position(&drawn.place))) > TREMBLE_M
            || turned
            || (0..2).any(|i| (self.half[i] - drawn.half[i]).abs() > 1e-4)
            || (0..4).any(|i| (self.cover[i] - drawn.cover[i]).abs() > 1e-4)
            || (self.depth - drawn.depth).abs() > TREMBLE_M
            || self.look != drawn.look
    }
}

/// Where a quad lies (`frame` its middle, `width` by `height` metres) from a hole's middle `at`, in the hole's plane: min x, min y, max x,
/// max y. The sheet's opaque picture is the back's quad.
pub fn rect_from(at: &Matrix34, frame: &Matrix34, width: f32, height: f32) -> [f32; 4] {
    let d = sub(position(frame), position(at));
    let (x, y) = (dot(d, [at.m[0][0], at.m[1][0], at.m[2][0]]), dot(d, [at.m[0][1], at.m[1][1], at.m[2][1]]));
    [x - width / 2.0, y - height / 2.0, x + width / 2.0, y + height / 2.0]
}

/// A premultiplied picture's colour `over` the backdrop `under`: what the splats leave of it shows, and the texel is whole.
pub fn over(c: [f32; 4], under: [f32; 3]) -> [f32; 4] {
    let left = 1.0 - c[3];
    [c[0] + left * under[0], c[1] + left * under[1], c[2] + left * under[2], 1.0]
}

/// The rays from `eye` through face `f`'s texels of `cube`: direction = A · (x, y, 1), pixel centres at integers (the columns of A).
/// As [`Cube::homography`] has them.
pub fn texel_rays(cube: &Cube, f: usize, eye: V3) -> [V3; 3] {
    let face: Face = FACES[f];
    let (right, down) = (face.right(), scale(face.up, -1.0));
    let w = cube.width() as f32;
    let step = 2.0 * cube.extent() / w;
    let corner = add(add(sub(add(cube.center, scale(face.normal, cube.half)), eye), scale(right, cube.extent() * (1.0 / w - 1.0))), scale(down, cube.extent() * (1.0 / w - 1.0)));
    [scale(right, step), scale(down, step), corner]
}

/// The bytes of one dispatch of `resample.wgsl` (its `Params`, 256 bytes): the homography `h` from a face's texels into the picture
/// read, `size` (face width, picture width and height, the eye's first column), and, with an opening, the rays of the face's texels from
/// `eye` ([`texel_rays`]). `whole`: the picture read is the face's own, texel for texel.
pub fn params(h: &[[f32; 3]; 3], size: [u32; 4], whole: bool, backdrop: Option<(&Opening, [V3; 3], V3)>) -> [u8; 256] {
    let mut floats = [0f32; 64];
    for (r, row) in h.iter().enumerate() {
        floats[r * 4..r * 4 + 3].copy_from_slice(row);
    }
    let mut put = |at: usize, v: V3, w: f32| {
        floats[at..at + 3].copy_from_slice(&v);
        floats[at + 3] = w;
    };
    put(32, [0.0; 3], if whole { 1.0 } else { 0.0 });
    if let Some((opening, rays, eye)) = backdrop {
        let look = &opening.look;
        put(16, rays[0], 0.0);
        put(20, rays[1], 0.0);
        put(24, rays[2], 0.0);
        put(28, eye, 1.0);
        put(32, position(&opening.place), if whole { 1.0 } else { 0.0 });
        put(36, opening.axis(0), opening.half[0]);
        put(40, opening.axis(1), opening.half[1]);
        put(44, opening.axis(2), opening.depth);
        put(48, [opening.cover[0], opening.cover[1], opening.cover[2]], opening.cover[3]);
        put(52, look.top, if look.flat { 1.0 } else { 0.0 });
        put(56, look.horizon, opening.spread());
        put(60, look.glow, 0.0);
    }
    let mut bytes = [0u8; 256];
    for (i, v) in floats.iter().enumerate() {
        bytes[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
    }
    // The sizes are integers, in the floats' fourth row.
    for (i, v) in size.iter().enumerate() {
        bytes[48 + i * 4..52 + i * 4].copy_from_slice(&v.to_le_bytes());
    }
    bytes
}

/// What the eye sees through texel `texel` of a face whose picture there is `c` (premultiplied): the picture over the backdrop where the
/// opening covers, nothing where it does not (the capture is seen through its window only: from aside, what lies behind the sheet showed
/// past its edge, G1's smear at the frame's right side), as the GPU makes it. Without an opening, the picture as it is.
pub fn texel(c: [f32; 4], opening: Option<&Opening>, eye: V3, dir: V3, texel: [u32; 2]) -> [f32; 4] {
    let Some(o) = opening else { return c };
    match o.colour(eye, dir, texel) {
        Some(under) => over(c, under),
        None => [0.0; 4],
    }
}

#[cfg(test)]
mod tests {
    use super::super::stereo::{SmallDock, StartView};
    use super::*;

    const IPD: f32 = 0.064;

    /// A sheet held half a metre before the eyes, a little below them, turned up to them, its hole as the Worlds page has it (VR_HERO in a
    /// 1024 × 576 page 0.634 m wide), the back with its head 0.501 m high around it: the numbers of the app at size 1.
    fn sheet() -> (Opening, [f32; 4], SmallDock) {
        let (page_w, page_h) = (0.6336, 0.6336 * 576.0 / 1024.0);
        let hole = [464.0f32, 24.0, 1000.0, 250.0];
        let (cx, cy) = (((hole[0] + hole[2]) / 2.0 / 1024.0 - 0.5) * page_w, (0.5 - (hole[1] + hole[3]) / 2.0 / 576.0) * page_h);
        let half = [(hole[2] - hole[0]) / 1024.0 * page_w / 2.0, (hole[3] - hole[1]) / 576.0 * page_h / 2.0];
        // Tipped 20 degrees back, its middle 0.5 m ahead and 0.25 m under the eyes.
        let (s, c) = 20f32.to_radians().sin_cos();
        let page = Matrix34 { m: [[1.0, 0.0, 0.0, -0.05], [0.0, c, s, 1.35], [0.0, -s, c, -0.5]] };
        let place = super::super::stereo::compose(&page, &Matrix34 { m: [[1.0, 0.0, 0.0, cx], [0.0, 1.0, 0.0, cy], [0.0, 0.0, 1.0, 0.0]] });
        // The back: as wide as the page, from 44 units over its top (the head) to 5.5 under its foot, in quick-menu units of 0.9 mm.
        let unit = page_w / 704.0;
        let back = [-page_w / 2.0 - cx, -page_h / 2.0 - 5.5 * unit - cy, page_w / 2.0 - cx, page_h / 2.0 + 44.0 * unit - cy];
        let radius = half[0] * super::super::SMALL_FILLS_HOLE;
        let view = StartView { position: [0.0, 1.6, 0.0], target: [0.0, 1.6, -1.0], floor: Some(0.0), eye_height: Some(1.6) };
        let dock = SmallDock::behind(&view, &place, radius, 4.0);
        (Opening::in_sheet(place, half, back, radius, Look::ours([0x00, 0xda, 0x6d], false)), back, dock)
    }

    /// Both eyes of a head at `at`, looking at `target`.
    fn eyes(at: V3, target: V3) -> [V3; 2] {
        let ahead = sub(target, at);
        let across = [-ahead[2], 0.0, ahead[0]];
        let across = scale(across, 1.0 / length(across));
        [add(at, scale(across, -IPD / 2.0)), add(at, scale(across, IPD / 2.0))]
    }

    /// Where the head is seen from: before it, from either side as far as the hole can still be looked into, from over and under it.
    fn heads(o: &Opening) -> Vec<(&'static str, [V3; 2])> {
        let mid = position(&o.place);
        let out = [o.place.m[0][2], o.place.m[1][2], o.place.m[2][2]];
        let across = [o.place.m[0][0], o.place.m[1][0], o.place.m[2][0]];
        let up = [o.place.m[0][1], o.place.m[1][1], o.place.m[2][1]];
        let from = |name, degrees_across: f32, degrees_up: f32, metres: f32| {
            let (sa, ca) = degrees_across.to_radians().sin_cos();
            let (su, cu) = degrees_up.to_radians().sin_cos();
            let dir = add(add(scale(out, ca * cu), scale(across, sa * cu)), scale(up, su));
            let at = add(mid, scale(dir, metres));
            (name, eyes(at, mid))
        };
        vec![
            from("before it", 0.0, 0.0, 0.5),
            from("near", 0.0, 0.0, 0.25),
            from("far", 0.0, 0.0, 0.9),
            from("from the left", -40.0, 0.0, 0.5),
            from("from the right", 40.0, 0.0, 0.5),
            from("grazing, left", -65.0, 0.0, 0.5),
            from("grazing, right", 65.0, 5.0, 0.5),
            from("from over it", 0.0, 45.0, 0.5),
            from("from under it", 0.0, -40.0, 0.5),
        ]
    }

    /// Rays through points across the whole hole, its very edges included.
    fn through_the_hole(o: &Opening, eye: V3) -> Vec<V3> {
        let mut rays = Vec::new();
        let across = [o.place.m[0][0], o.place.m[1][0], o.place.m[2][0]];
        let up = [o.place.m[0][1], o.place.m[1][1], o.place.m[2][1]];
        for i in 0..=24 {
            for j in 0..=12 {
                let (a, b) = (-1.0 + 2.0 * i as f32 / 24.0, -1.0 + 2.0 * j as f32 / 12.0);
                let p = add(add(position(&o.place), scale(across, a * o.half[0] * 0.9999)), scale(up, b * o.half[1] * 0.9999));
                rays.push(sub(p, eye));
            }
        }
        rays
    }

    /// Enzo's G1: through the hole, VRChat's menu. Now every ray any eye sends through any point of the hole, from any angle the hole can be
    /// looked into, meets our backdrop, whole: nothing of what is behind the sheet shows.
    #[test]
    fn the_backdrop_covers_the_whole_opening_from_every_angle_for_both_eyes() {
        let (o, _, _) = sheet();
        for (name, pair) in heads(&o) {
            for (e, eye) in pair.iter().enumerate() {
                for dir in through_the_hole(&o, *eye) {
                    // A texel the splats left empty: only the backdrop shows there.
                    let seen = texel([0.0; 4], Some(&o), *eye, dir, [3, 5]);
                    assert_eq!(seen[3], 1.0, "{name}, eye {e}: a ray through the hole sees through it");
                    // The layer missing (dev.40): what is behind shows.
                    assert_eq!(texel([0.0; 4], None, *eye, dir, [3, 5])[3], 0.0);
                }
            }
        }
    }

    /// The backdrop is behind the capture, never over it (F7): the splats' colour is kept where they cover, whatever the backdrop, and the
    /// backdrop only fills what they leave.
    #[test]
    fn the_capture_is_in_front_of_the_backdrop() {
        let (o, _, dock) = sheet();
        let [eye, _] = eyes([0.0, 1.6, 0.0], position(&o.place));
        let dir = sub(position(&o.place), eye);
        let under = o.colour(eye, dir, [0, 0]).expect("covered");
        let splat = [0.8, 0.3, 0.1, 1.0];
        assert_eq!(texel(splat, Some(&o), eye, dir, [0, 0]), splat, "an opaque splat is untouched");
        let half = [0.4, 0.15, 0.05, 0.5];
        let seen = texel(half, Some(&o), eye, dir, [0, 0]);
        for c in 0..3 {
            assert!((seen[c] - (half[c] + 0.5 * under[c])).abs() < 1e-6, "half a splat over half the backdrop");
            assert!(seen[c] >= half[c], "nothing of the splat is taken away");
        }
        // In depth too: the plane it lies on is behind all of the capture's round, which lies one radius behind the hole and reaches one more.
        let ez = [o.place.m[0][2], o.place.m[1][2], o.place.m[2][2]];
        let capture_back = -dot(sub(dock.anchor, position(&o.place)), ez) + dock.radius;
        assert!(o.depth > capture_back * 1.2, "backdrop {} m behind the hole, the capture to {capture_back} m", o.depth);
        // And for both eyes: a point of the backdrop is seen with the disparity of its depth, farther than the capture's middle.
        let pair = eyes([0.0, 1.6, 0.0], position(&o.place));
        let back_point = sub(position(&o.place), scale(ez, o.depth));
        let angle = |p: V3| {
            let (a, b) = (sub(p, pair[0]), sub(p, pair[1]));
            (dot(a, b) / length(a) / length(b)).clamp(-1.0, 1.0).acos()
        };
        assert!(angle(back_point) < angle(dock.anchor), "the eyes converge farther for the backdrop than for the capture");
    }

    /// The capture lies behind the sheet and reaches past its sides: seen from aside it showed past the sheet's right edge (only 15 mm of
    /// sheet there), in the cube's coarse texels (G1's smear and staircase at the frame's right side). In its hole it is seen through its
    /// window only: a splat on any ray that does not go through what the backdrop covers is not drawn, from any side; through the hole it is.
    #[test]
    fn the_capture_is_seen_through_its_window_only() {
        let (o, back, _) = sheet();
        let across = [o.place.m[0][0], o.place.m[1][0], o.place.m[2][0]];
        let up = [o.place.m[0][1], o.place.m[1][1], o.place.m[2][1]];
        let splat = [0.7, 0.6, 0.5, 1.0];
        for (name, pair) in heads(&o) {
            for eye in pair {
                // Just past the sheet's right edge and over its head, and on the sheet away from the hole.
                for (u, v) in [(back[2] + 0.004, 0.0), (back[2] + 0.03, -0.05), (0.0, back[3] + 0.01), (back[0] + 0.05, 0.0), (0.0, back[1] + 0.03)] {
                    let p = add(add(position(&o.place), scale(across, u)), scale(up, v));
                    assert_eq!(texel(splat, Some(&o), eye, sub(p, eye), [1, 1]), [0.0; 4], "{name}: the capture past the window at {u:.3},{v:.3}");
                    // As dev.40 had it: there.
                    assert_eq!(texel(splat, None, eye, sub(p, eye), [1, 1]), splat);
                }
                for dir in through_the_hole(&o, eye) {
                    assert_eq!(texel(splat, Some(&o), eye, dir, [1, 1]), splat, "{name}: the capture through its hole");
                }
            }
        }
    }

    /// Never past the sheet: a ray that meets the sheet's plane outside the sheet's opaque picture never meets the backdrop, so it never
    /// shows as a slab around the menu, from any side; and what it covers holds the whole hole.
    #[test]
    fn the_backdrop_stays_under_the_sheet() {
        let (o, back, _) = sheet();
        assert!(o.cover[0] <= -o.half[0] && o.cover[1] <= -o.half[1] && o.cover[2] >= o.half[0] && o.cover[3] >= o.half[1], "{:?} holds the hole", o.cover);
        assert!(o.cover[0] >= back[0] + INSET_M - 1e-6 && o.cover[1] >= back[1] + INSET_M - 1e-6, "{:?} inside {back:?}", o.cover);
        assert!(o.cover[2] <= back[2] - INSET_M + 1e-6 && o.cover[3] <= back[3] - INSET_M + 1e-6, "{:?} inside {back:?}", o.cover);
        // Grown where the sheet has room: on the left, over and under the hole by GROW_M.
        assert!((o.cover[0] + o.half[0] + GROW_M).abs() < 1e-6 && (o.cover[1] + o.half[1] + GROW_M).abs() < 1e-6);
        let across = [o.place.m[0][0], o.place.m[1][0], o.place.m[2][0]];
        let up = [o.place.m[0][1], o.place.m[1][1], o.place.m[2][1]];
        for (name, pair) in heads(&o) {
            for eye in pair {
                for i in 0..=40 {
                    for j in 0..=40 {
                        let (u, v) = (back[0] - 0.1 + (back[2] - back[0] + 0.2) * i as f32 / 40.0, back[1] - 0.1 + (back[3] - back[1] + 0.2) * j as f32 / 40.0);
                        let p = add(add(position(&o.place), scale(across, u)), scale(up, v));
                        let on_sheet = u >= back[0] && u <= back[2] && v >= back[1] && v <= back[3];
                        if !on_sheet {
                            assert!(o.hit(eye, sub(p, eye)).is_none(), "{name}: backdrop past the sheet at {u:.3},{v:.3}");
                        }
                    }
                }
            }
        }
        // A sheet whose edge is nearer the hole than GROW_M (the hole's right side): it stops before that edge, and still holds the hole.
        let tight = Opening::in_sheet(o.place, o.half, [back[0], back[1], o.half[0] + 0.004, back[3]], 0.3, o.look);
        assert!((tight.cover[2] - (o.half[0] + 0.004 - INSET_M)).abs() < 1e-6, "{:?}: up to the sheet's edge less its inset", tight.cover);
        let tighter = Opening::in_sheet(o.place, o.half, [back[0], back[1], o.half[0] + 0.001, back[3]], 0.3, o.look);
        assert_eq!(tighter.cover[2], o.half[0], "never less than the hole");
        // Near a side, it keeps clear of the sheet's round corners: a sheet whose top is just over the hole.
        let low = Opening::in_sheet(o.place, o.half, [back[0], back[1], back[2], o.half[1] + 0.03], 0.3, o.look);
        assert!(low.cover[2] > back[2] - SHEET_CORNER_M && low.cover[3] <= o.half[1] + 0.03 - SHEET_CORNER_M + 1e-6, "{:?}: into the sheet's corner", low.cover);
    }

    /// The panel moves in a hand between two pictures: what was drawn for its last place still fills the hole where it is now, for a move
    /// as large as the sheet's narrowest edge allows (the hole's right side: 15 mm of page, less the inset).
    #[test]
    fn a_hand_moving_the_panel_between_two_pictures_leaves_no_gap() {
        let (drawn, back, _) = sheet();
        let room = (back[2] - drawn.half[0] - INSET_M).min(GROW_M);
        assert!(room > 0.007, "{room} m of room on the narrowest side");
        // Across its plane as far as that room; towards or away from the eyes 7.5 mm (at 0.2 m/s within one batch), seen up to 40 degrees
        // aside, where that move slides the hole's edge across the old plane by 1.3 times as much.
        for (dx, dy, dz, moved_by) in [(1.0, 0.0, 0.0, room * 0.85), (-1.0, 0.0, 0.0, room * 0.85), (0.0, 1.0, 0.0, room * 0.85), (0.0, -1.0, 0.0, room * 0.85), (0.7, 0.7, 0.0, room * 0.85), (0.0, 0.0, 1.0, 0.0075), (0.0, 0.0, -1.0, 0.0075)] {
            let mut now = drawn;
            let shift = add(add(scale([drawn.place.m[0][0], drawn.place.m[1][0], drawn.place.m[2][0]], dx * moved_by), scale([drawn.place.m[0][1], drawn.place.m[1][1], drawn.place.m[2][1]], dy * moved_by)), scale([drawn.place.m[0][2], drawn.place.m[1][2], drawn.place.m[2][2]], dz * moved_by));
            for r in 0..3 {
                now.place.m[r][3] += shift[r];
            }
            for (name, pair) in heads(&now).into_iter().filter(|(n, _)| !n.starts_with("grazing")) {
                for eye in pair {
                    for dir in through_the_hole(&now, eye) {
                        assert!(drawn.hit(eye, dir).is_some(), "{name}: moved {dx},{dy},{dz} by {moved_by} m, a gap at the frame");
                    }
                }
            }
        }
    }

    /// Its look: darker than the sheet's own ink around it (theme INK_800, 0x1f), the accent only a glow, a gradient with no step of more
    /// than the dither, a flat ink in the retro look; and steady as a hand trembles.
    #[test]
    fn the_backdrop_is_calm_and_darker_than_the_sheet() {
        let (o, _, _) = sheet();
        let [eye, _] = eyes([0.0, 1.6, 0.0], position(&o.place));
        let mut last: Option<[f32; 3]> = None;
        let up = [o.place.m[0][1], o.place.m[1][1], o.place.m[2][1]];
        for j in 0..=100 {
            let p = add(position(&o.place), scale(up, -o.half[1] + 2.0 * o.half[1] * j as f32 / 100.0));
            let c = o.colour(eye, sub(p, eye), [7, 7]).expect("covered");
            let luma = |c: [f32; 3]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
            assert!(luma(c) < luma([0x1f as f32 / 255.0, 0x1f as f32 / 255.0, 0x23 as f32 / 255.0]), "{c:?} darker than the panel round the hole");
            assert!(c[1] - c[0].max(c[2]) < 0.04, "{c:?}: the accent is a glow, not a colour");
            if let Some(l) = last {
                assert!((0..3).all(|i| (c[i] - l[i]).abs() < 3.0 / 255.0), "{l:?} to {c:?}: a step");
            }
            last = Some(c);
        }
        let retro = Opening { look: Look::ours([185; 3], true), ..o };
        let flat: Vec<_> = (0..5).map(|i| retro.colour(eye, sub(position(&o.place), add(eye, [0.01 * i as f32, 0.0, 0.0])), [i, i]).unwrap()).collect();
        assert!(flat.windows(2).all(|w| w[0] == w[1]), "one flat ink in the retro look");
        let mut trembled = o;
        trembled.place.m[0][3] += 0.002;
        assert!(!trembled.moved_from(&o), "a trembling hand draws nothing again");
        trembled.place.m[0][3] += 0.004;
        assert!(trembled.moved_from(&o), "a real move does");
    }

    /// What the next batch draws behind the capture: the panel's opening while the capture lies in its hole, none once a hand took it or the
    /// menu closed (the view then draws every face again whole), and the one drawn last while the panel only trembles.
    #[test]
    fn the_view_draws_the_opening_only_while_the_capture_lies_in_its_hole() {
        use super::super::view::opening_for;
        let (o, _, _) = sheet();
        assert_eq!(opening_for(Some(o), true, None), Some(o), "in its hole: the backdrop");
        assert_eq!(opening_for(Some(o), false, Some(o)), None, "taken in a hand: frameless, nothing behind it");
        assert_eq!(opening_for(None, true, Some(o)), None, "the menu closed: no hole, no backdrop");
        let mut trembled = o;
        trembled.place.m[1][3] += 0.001;
        assert_eq!(opening_for(Some(trembled), true, Some(o)), Some(o), "a trembling hand: the same opening, nothing drawn again");
        let mut moved = o;
        moved.place.m[1][3] += 0.01;
        assert_eq!(opening_for(Some(moved), true, Some(o)), Some(moved), "moved: drawn where it is now");
    }

    /// The texels' rays are the homography's: a face texel's ray, seen through the wide camera that drew it, lands on the pixel the
    /// homography gives.
    #[test]
    fn texel_rays_are_the_homography_s() {
        use super::super::stereo::PlayCamera;
        let cube = Cube { center: [0.0, 1.6, 0.0], half: 1.5, pixels: 56, margin: 4 };
        let eye = [0.03, 1.62, -0.01];
        let view = Matrix34 { m: [[1.0, 0.0, 0.0, eye[0]], [0.0, 1.0, 0.0, eye[1]], [0.0, 0.0, 1.0, eye[2]]] };
        let wide = PlayCamera::wide(&view, [-1.0, 1.0, -1.0, 1.0], 0.1, 40.0, 512);
        let rays = texel_rays(&cube, 0, eye);
        let h = cube.homography(0, &wide);
        for (x, y) in [(0.0, 0.0), (10.0, 50.0), (63.0, 63.0), (31.5, 20.0)] {
            let dir = add(add(scale(rays[0], x), scale(rays[1], y)), rays[2]);
            let p = wide.project(add(eye, dir)).unwrap();
            let t = [x, y, 1.0];
            let q = [dot(h[0], t), dot(h[1], t), dot(h[2], t)];
            assert!((p[0] - q[0] / q[2]).abs() < 1e-2 && (p[1] - q[1] / q[2]).abs() < 1e-2, "{p:?} against {q:?}");
        }
    }

    /// The bytes the shader reads: its rows where `Params` has them.
    #[test]
    fn the_shader_s_params_are_laid_as_it_reads_them() {
        let (o, _, _) = sheet();
        let h = [[1.0, 2.0, 3.0], [4.0, 5.0, 6.0], [7.0, 8.0, 9.0]];
        let b = params(&h, [64, 100, 80, 64], true, Some((&o, [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.5, 0.5, -1.0]], [0.0, 1.6, 0.0])));
        let f = |i: usize| f32::from_le_bytes(b[i * 4..i * 4 + 4].try_into().unwrap());
        let u = |i: usize| u32::from_le_bytes(b[i * 4..i * 4 + 4].try_into().unwrap());
        assert_eq!([f(0), f(5), f(10)], [1.0, 5.0, 9.0]);
        assert_eq!([u(12), u(13), u(14), u(15)], [64, 100, 80, 64]);
        assert_eq!([f(24), f(25), f(26)], [0.5, 0.5, -1.0]);
        assert_eq!([f(29), f(31)], [1.6, 1.0], "the eye and that there is a backdrop");
        assert_eq!(f(35), 1.0, "texel for texel");
        assert_eq!([f(39), f(43), f(47)], [o.half[0], o.half[1], o.depth]);
        assert_eq!([f(48), f(51)], [o.cover[0], o.cover[3]]);
        assert_eq!(f(59), o.spread());
        let none = params(&h, [64, 100, 80, 0], false, None);
        assert_eq!(f32::from_le_bytes(none[124..128].try_into().unwrap()), 0.0, "no backdrop");
    }
}
