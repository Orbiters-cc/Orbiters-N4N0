//! Prints each session's scene extent (crate::train::extent: the larger of the cameras' spread and the median sampled
//! depth), which scales the position learning rate and so 3DGS-MCMC's SGLD noise (train::Settings::noise):
//!
//! ```text
//! extent <session folder>...
//! ```

use orbiters_reconstruct::{prepare, train};

fn main() -> anyhow::Result<()> {
    for dir in std::env::args().skip(1) {
        let p = prepare::prepare(std::path::Path::new(&dir), &prepare::Options::default(), |_| {})?;
        let d = &p.data;
        let c = train::camera_centre(d);
        let spread = d.views.iter().map(|v| v.camera.position()).map(|q| ((q[0] - c[0]).powi(2) + (q[1] - c[1]).powi(2) + (q[2] - c[2]).powi(2)).sqrt()).fold(0.0f32, f32::max);
        println!("{dir}: extent {:.2} m (cameras within {:.2} m of their centre, {} views)", train::extent(d), spread, d.views.len());
    }
    Ok(())
}
