//! Pinhole cameras from the capture strip. The world is Unity's (left-handed, metres); camera space is OpenCV's
//! (+X right, +Y down, +Z forward), so the world-to-camera map includes a reflection: its rotation part has
//! determinant −1, which the maths below never assumes otherwise.

use orbiters_contracts::coords::intrinsics_from_projection;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    /// World to camera: `p_c = r · p_w + t` (rows are the camera's right, down and forward axes in the world).
    pub r: [[f32; 3]; 3],
    pub t: [f32; 3],
    pub fx: f32,
    pub fy: f32,
    /// Principal point; pixel (0, 0) is the centre of the top-left pixel.
    pub cx: f32,
    pub cy: f32,
    pub width: u32,
    pub height: u32,
}

/// The training resolution of a capture: the captured frame box-filtered by an integer `factor`, its last
/// `full_width % factor` columns and `full_height % factor` rows left out. Keyframes, cameras, depth-sample positions
/// and the loss mask all follow it, so they line up pixel for pixel whatever the capture's size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Geometry {
    pub full_width: u32,
    pub full_height: u32,
    pub factor: u32,
}

impl Default for Geometry {
    fn default() -> Self {
        Self { full_width: 0, full_height: 0, factor: 1 }
    }
}

impl Geometry {
    pub fn new(full_width: u32, full_height: u32, factor: u32) -> Self {
        Self { full_width, full_height, factor: factor.max(1) }
    }

    /// The largest training size at most `max_width` wide (64 at least) that divides the capture by an integer.
    pub fn at_most(full_width: u32, full_height: u32, max_width: u32) -> Self {
        Self::new(full_width, full_height, full_width.div_ceil(max_width.max(64)).max(1))
    }

    pub fn width(&self) -> u32 {
        self.full_width / self.factor
    }

    pub fn height(&self) -> u32 {
        self.full_height / self.factor
    }

    /// A captured pixel coordinate (pixel centres at integers) at the training resolution: training pixel `x` averages
    /// captured pixels `factor·x .. factor·x + factor − 1`.
    pub fn to_training(&self, x: f32) -> f32 {
        (x + 0.5) / self.factor as f32 - 0.5
    }

    /// The training pixels a captured rectangle (half-open, captured pixels) touches, clipped to the image.
    pub fn cover(&self, x0: u32, y0: u32, x1: u32, y1: u32) -> (u32, u32, u32, u32) {
        let f = self.factor;
        ((x0 / f).min(self.width()), (y0 / f).min(self.height()), x1.div_ceil(f).min(self.width()), y1.div_ceil(f).min(self.height()))
    }

    /// The training camera of a strip pose (Unity camera-to-world, row-major 3×4) and projection terms.
    pub fn camera(&self, camera_to_world: &[f32; 12], projection: &[f32; 4]) -> Camera {
        Camera::from_strip(camera_to_world, projection, self.full_width, self.full_height, 1.0).scaled(self.factor)
    }
}

impl Camera {
    /// From a strip pose (Unity camera-to-world, row-major 3×4) and projection terms, for an image of
    /// `width × height` scaled by `scale` about the pixel centres, its size rounded (a viewer's camera; training
    /// cameras come from [`Geometry::camera`], which crops like the keyframes do).
    pub fn from_strip(camera_to_world: &[f32; 12], projection: &[f32; 4], width: u32, height: u32, scale: f32) -> Self {
        let m = camera_to_world;
        let right = [m[0], m[4], m[8]];
        let up = [m[1], m[5], m[9]];
        let forward = [m[2], m[6], m[10]];
        let r = [right, [-up[0], -up[1], -up[2]], forward];
        let pos = [m[3], m[7], m[11]];
        let t = [-dot(r[0], pos), -dot(r[1], pos), -dot(r[2], pos)];
        let k = intrinsics_from_projection(projection, width, height);
        let w = ((width as f32) * scale).round().max(1.0) as u32;
        let h = ((height as f32) * scale).round().max(1.0) as u32;
        Self { r, t, fx: k.fx * scale, fy: k.fy * scale, cx: (k.cx + 0.5) * scale - 0.5, cy: (k.cy + 0.5) * scale - 0.5, width: w, height: h }
    }

    /// A camera at `eye` looking along `forward`, `up` roughly up, with a vertical field of view of `fov_y` radians,
    /// for an image of `width × height` (a viewer's free camera).
    pub fn look(eye: [f32; 3], forward: [f32; 3], up: [f32; 3], fov_y: f32, width: u32, height: u32) -> Self {
        let f = normalize(forward);
        let right = normalize(cross(up, f));
        let up = cross(f, right);
        let r = [right, [-up[0], -up[1], -up[2]], f];
        let t = [-dot(r[0], eye), -dot(r[1], eye), -dot(r[2], eye)];
        let focal = 0.5 * height as f32 / (0.5 * fov_y).tan();
        Self { r, t, fx: focal, fy: focal, cx: (width as f32 - 1.0) / 2.0, cy: (height as f32 - 1.0) / 2.0, width, height }
    }

    /// The camera of this image box-filtered by an integer `factor` (pixel centres at integers on both; the last
    /// `width % factor` columns and `height % factor` rows left out).
    pub fn scaled(&self, factor: u32) -> Camera {
        let factor = factor.max(1);
        let f = factor as f32;
        Camera {
            r: self.r,
            t: self.t,
            fx: self.fx / f,
            fy: self.fy / f,
            cx: (self.cx + 0.5) / f - 0.5,
            cy: (self.cy + 0.5) / f - 0.5,
            width: self.width / factor,
            height: self.height / factor,
        }
    }

    /// Vertical field of view in radians.
    pub fn fov_y(&self) -> f32 {
        2.0 * (0.5 * self.height as f32 / self.fy).atan()
    }

    /// The camera's up axis in the world.
    pub fn up(&self) -> [f32; 3] {
        [-self.r[1][0], -self.r[1][1], -self.r[1][2]]
    }

    pub fn to_camera(&self, p: [f32; 3]) -> [f32; 3] {
        [dot(self.r[0], p) + self.t[0], dot(self.r[1], p) + self.t[1], dot(self.r[2], p) + self.t[2]]
    }

    /// Camera-space point back to the world (the rotation part is orthogonal: its inverse is its transpose).
    pub fn to_world(&self, c: [f32; 3]) -> [f32; 3] {
        let d = [c[0] - self.t[0], c[1] - self.t[1], c[2] - self.t[2]];
        [
            self.r[0][0] * d[0] + self.r[1][0] * d[1] + self.r[2][0] * d[2],
            self.r[0][1] * d[0] + self.r[1][1] * d[1] + self.r[2][1] * d[2],
            self.r[0][2] * d[0] + self.r[1][2] * d[1] + self.r[2][2] * d[2],
        ]
    }

    /// The camera's centre in the world.
    pub fn position(&self) -> [f32; 3] {
        self.to_world([0.0, 0.0, 0.0])
    }

    /// The camera's forward axis in the world.
    pub fn forward(&self) -> [f32; 3] {
        self.r[2]
    }

    /// Pixel coordinates and depth of a world point, or `None` behind the camera.
    pub fn project(&self, p: [f32; 3]) -> Option<(f32, f32, f32)> {
        let c = self.to_camera(p);
        (c[2] > 1e-4).then(|| (self.fx * c[0] / c[2] + self.cx, self.fy * c[1] / c[2] + self.cy, c[2]))
    }

    /// The world point at pixel (`u`, `v`) and depth `z`.
    pub fn unproject(&self, u: f32, v: f32, z: f32) -> [f32; 3] {
        self.to_world([(u - self.cx) / self.fx * z, (v - self.cy) / self.fy * z, z])
    }
}

pub fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

pub fn normalize(a: [f32; 3]) -> [f32; 3] {
    let l = dot(a, a).sqrt().max(1e-12);
    a.map(|c| c / l)
}

pub fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn look_matches_a_strip_pose() {
        // A camera turned 30° right and tilted 10° down, at (1, 1.6, -2).
        let (yaw, pitch) = (30f32.to_radians(), -10f32.to_radians());
        let forward = [yaw.sin() * pitch.cos(), pitch.sin(), yaw.cos() * pitch.cos()];
        let right = normalize(cross([0.0, 1.0, 0.0], forward));
        let up = cross(forward, right);
        let m = [right[0], up[0], forward[0], 1.0, right[1], up[1], forward[1], 1.6, right[2], up[2], forward[2], -2.0];
        let strip = Camera::from_strip(&m, &[1.2, 1.2 * 16.0 / 9.0, 0.0, 0.0], 320, 180, 1.0);
        let look = Camera::look([1.0, 1.6, -2.0], forward, [0.0, 1.0, 0.0], strip.fov_y(), 320, 180);
        for k in 0..3 {
            for c in 0..3 {
                assert!((strip.r[k][c] - look.r[k][c]).abs() < 1e-5, "{strip:?} {look:?}");
            }
            assert!((strip.t[k] - look.t[k]).abs() < 1e-5);
        }
        assert!((strip.fy - look.fy).abs() < 1e-2 && (strip.cx - look.cx).abs() < 1e-3 && (strip.cy - look.cy).abs() < 1e-3, "{strip:?} {look:?}");
    }

    #[test]
    fn matches_the_contracts_projection_and_round_trips() {
        let a = 0.4f32;
        let pose = [a.cos(), 0.0, a.sin(), 1.0, 0.0, 1.0, 0.0, 1.6, -a.sin(), 0.0, a.cos(), -2.0];
        let proj = [0.974_278_6, 1.732_050_8, 0.01, -0.02];
        let cam = Camera::from_strip(&pose, &proj, 1920, 1080, 1.0);
        let p = [1.5, 2.0, 3.0];
        let (u, v, z) = cam.project(p).unwrap();
        let (u2, v2, z2) = orbiters_contracts::coords::project_unity(p, &pose, &proj, 1920, 1080).unwrap();
        assert!((u - u2).abs() < 1e-3 && (v - v2).abs() < 1e-3 && (z - z2).abs() < 1e-5);
        let back = cam.unproject(u, v, z);
        assert!(p.iter().zip(back).all(|(a, b)| (a - b).abs() < 1e-4), "{back:?}");
        assert!(cam.position().iter().zip([1.0, 1.6, -2.0]).all(|(a, b)| (a - b).abs() < 1e-5));
        // Half resolution: the same point lands at half the pixel coordinates (pixel centres preserved).
        let half = Camera::from_strip(&pose, &proj, 1920, 1080, 0.5);
        let (uh, vh, _) = half.project(p).unwrap();
        assert!(((uh + 0.5) * 2.0 - (u + 0.5)).abs() < 1e-3 && ((vh + 0.5) * 2.0 - (v + 0.5)).abs() < 1e-3);
        assert_eq!((half.width, half.height), (960, 540));
    }

    /// Capture sizes the factor does not divide: the training camera is exactly the size of the cropped, box-filtered
    /// keyframe, and its pixel centres sit where the captured pixels they average are.
    #[test]
    fn training_cameras_match_cropped_keyframes_at_odd_capture_sizes() {
        let a = 0.3f32;
        let pose = [a.cos(), 0.0, a.sin(), 1.0, 0.0, 1.0, 0.0, 1.6, -a.sin(), 0.0, a.cos(), -2.0];
        for (w, h, max) in [(2560u32, 1441u32, 1280u32), (2561, 1440, 1280), (1923, 1083, 640), (1920, 1080, 1920)] {
            let g = Geometry::at_most(w, h, max);
            let full = Camera::from_strip(&pose, &[1.2, 1.2 * w as f32 / h as f32, 0.01, -0.02], w, h, 1.0);
            let cam = g.camera(&pose, &[1.2, 1.2 * w as f32 / h as f32, 0.01, -0.02]);
            assert_eq!((cam.width, cam.height), (g.width(), g.height()), "{w}×{h}");
            assert_eq!((g.width(), g.height()), (w / g.factor, h / g.factor));
            for (x, y) in [(0.0f32, 0.0f32), (811.3, 402.7), ((w - 1) as f32, (h - 1) as f32)] {
                let p = full.unproject(x, y, 4.0);
                let (u, v, _) = cam.project(p).expect("in front");
                assert!((u - g.to_training(x)).abs() < 2e-3 && (v - g.to_training(y)).abs() < 2e-3, "{w}×{h} at ({x}, {y}): {u} {v}");
            }
        }
        // The rectangle a captured area covers at the training resolution.
        let g = Geometry::new(1921, 1081, 2);
        assert_eq!(g.cover(0, 0, 3, 5), (0, 0, 2, 3));
        assert_eq!(g.cover(1900, 1070, 1921, 1081), (950, 535, 960, 540));
    }
}
