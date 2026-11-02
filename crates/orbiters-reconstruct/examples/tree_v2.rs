//! Lab only (sota/tree-v2): the same splat tree in version 1 and version 2, for the VRChat world's A/B harness.
//!
//! - `tree_v2 convert <v1 tree> <v2 out> [--coarse <v2 coarse out>]`: reads a version 1 file (32-byte records; the
//!   reader lives here only, no product code reads version 1 any more), writes the same boxes and splats as version 2
//!   with its key and flags, and prints a JSON line: sizes, and how far the version 2 records are from what version 1
//!   decoded to (positions per leaf and merged box, scales, rotations; colour and opacity bytes are the same). With
//!   `--coarse`, also the coarse file a Quest tree gives (its top 49 152 splats), and its version 1 size.
//! - `tree_v2 dump <v2 tree> <out.f32>`: what the world's SplatDecode shader should write for every splat, in Unity's
//!   world, as little-endian f32 × 12 a splat: position xyz, covariance xx xy xz yy yz zz, packed sRGB (r | g << 8 |
//!   b << 16, exact in f32), opacity (byte / 255), box; then f32 × 8 a box: sphere centre xyz (Unity), radius, error,
//!   parent (−1 for the root), first splat, splat count. Compared with the GPU's textures by the harness.
use orbiters_reconstruct::gaussians::Gaussians;
use orbiters_reconstruct::tree::{Node, Tree};
use orbiters_reconstruct::tree_tune::COARSE_SPLATS;

fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

fn f32_at(b: &[u8], o: usize) -> f32 {
    f32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

/// A version 1 tree (32-byte header, boxes and splats; the splats are the web `.splat` record), its key and flags.
fn read_v1(b: &[u8]) -> anyhow::Result<(Tree, u32, u32)> {
    anyhow::ensure!(b.len() >= 32 && &b[..8] == b"N4N0TREE" && u32_at(b, 8) == 1, "not a version 1 splat tree");
    let (boxes, splats) = (u32_at(b, 12) as usize, u32_at(b, 16) as usize);
    anyhow::ensure!(b.len() == 32 * (1 + boxes + splats), "a version 1 tree's length");
    let mut nodes: Vec<Node> = Vec::with_capacity(boxes);
    for i in 0..boxes {
        let at = (1 + i) * 32;
        let parent = (u32_at(b, at + 20) != u32::MAX).then(|| u32_at(b, at + 20));
        let depth = parent.map_or(0, |p| nodes[p as usize].depth + 1);
        nodes.push(Node {
            center: [f32_at(b, at), f32_at(b, at + 4), -f32_at(b, at + 8)],
            radius: f32_at(b, at + 12),
            error: f32_at(b, at + 16),
            parent,
            depth,
            start: u32_at(b, at + 24),
            count: u32_at(b, at + 28),
            leaf: true,
        });
        if let Some(p) = parent {
            nodes[p as usize].leaf = false;
        }
    }
    let g = orbiters_reconstruct::web::read_splat(&b[(1 + boxes) * 32..])?;
    Ok((Tree { nodes, splats: g }, u32_at(b, 20), u32_at(b, 24)))
}

fn angle(a: [f32; 4], b: [f32; 4]) -> f64 {
    let n = |q: [f32; 4]| q.iter().map(|v| (*v as f64).powi(2)).sum::<f64>().sqrt();
    let d = (0..4).map(|k| a[k] as f64 * b[k] as f64).sum::<f64>().abs() / (n(a) * n(b));
    2.0 * d.min(1.0).acos()
}

fn convert(args: &[String]) -> anyhow::Result<()> {
    anyhow::ensure!(args.len() == 2 || (args.len() == 4 && args[2] == "--coarse"), "tree_v2 convert <v1 tree> <v2 out> [--coarse <out>]");
    let bytes = std::fs::read(&args[0])?;
    let (tree, key, flags) = read_v1(&bytes)?;
    let v2 = tree.file(key, flags);
    std::fs::write(&args[1], &v2)?;
    let back = Tree::parse(&v2)?;
    anyhow::ensure!(back.nodes == tree.nodes, "the boxes come back");
    let (a, b) = (&tree.splats, &back.splats);
    // Worst and mean differences, per leaf and merged box.
    let mut stats = serde_json::Map::new();
    for leaf in [true, false] {
        let (mut pos_max, mut pos_sum, mut scale_max, mut rot_max, mut rot_sum, mut step_max, mut n) = (0f64, 0f64, 0f64, 0f64, 0f64, 0f64, 0usize);
        for (bi, node) in tree.nodes.iter().enumerate().filter(|(_, x)| x.leaf == leaf) {
            let at = orbiters_contracts::splat_tree::box_offset(bi);
            step_max = step_max.max((0..3).map(|k| f32_at(&v2, at + 44 + 4 * k) as f64).fold(0.0, f64::max));
            for i in node.start as usize..(node.start + node.count) as usize {
                let d = (0..3).map(|k| (a.means[i][k] - b.means[i][k]) as f64).map(f64::abs).fold(0.0, f64::max);
                pos_max = pos_max.max(d);
                pos_sum += d;
                for k in 0..3 {
                    scale_max = scale_max.max(((b.log_scales[i][k] - a.log_scales[i][k]) as f64).exp_m1().abs());
                }
                let r = angle(a.rotations[i], b.rotations[i]);
                rot_max = rot_max.max(r);
                rot_sum += r;
                n += 1;
                assert_eq!(a.colors[i], b.colors[i]);
                assert!((a.opacity_logits[i] - b.opacity_logits[i]).abs() < 1e-5);
            }
        }
        stats.insert(
            if leaf { "leaves".into() } else { "merged".into() },
            serde_json::json!({ "splats": n, "max_step_m": step_max, "max_position_m": pos_max, "mean_position_m": pos_sum / n.max(1) as f64,
                "max_scale_relative": scale_max, "max_rotation_rad": rot_max, "mean_rotation_rad": rot_sum / n.max(1) as f64 }),
        );
    }
    let mut line = serde_json::json!({ "tree": args[0], "boxes": tree.nodes.len(), "splats": tree.len(), "v1_bytes": bytes.len(), "v2_bytes": v2.len(),
        "ratio": bytes.len() as f64 / v2.len() as f64, "stats": stats });
    if args.len() == 4 {
        let coarse = tree.top(COARSE_SPLATS);
        let file = coarse.file(key, orbiters_reconstruct::tree::FLAG_COARSE);
        std::fs::write(&args[3], &file)?;
        line["coarse"] = serde_json::json!({ "boxes": coarse.nodes.len(), "splats": coarse.len(), "v1_bytes": 32 * (1 + coarse.nodes.len() + coarse.len()), "v2_bytes": file.len() });
    }
    println!("{line}");
    Ok(())
}

/// The covariance (xx, xy, xz, yy, yz, zz) of a splat in Unity's world, as SplatDecode computes it (in the file's frame,
/// then mirrored).
fn covariance(g: &Gaussians, i: usize) -> [f32; 6] {
    let q = g.rotations[i];
    // Back to the file's frame: (w, -x, -y, z).
    let n = q.iter().map(|v| (*v as f64).powi(2)).sum::<f64>().sqrt();
    let (w, x, y, z) = (q[0] as f64 / n, -q[1] as f64 / n, -q[2] as f64 / n, q[3] as f64 / n);
    let r = [
        [1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y - w * z), 2.0 * (x * z + w * y)],
        [2.0 * (x * y + w * z), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z - w * x)],
        [2.0 * (x * z - w * y), 2.0 * (y * z + w * x), 1.0 - 2.0 * (x * x + y * y)],
    ];
    let s = g.log_scales[i].map(|v| (v as f64).exp());
    let c = |a: usize, b: usize| (0..3).map(|k| r[a][k] * s[k] * r[b][k] * s[k]).sum::<f64>();
    [c(0, 0), c(0, 1), -c(0, 2), c(1, 1), -c(1, 2), c(2, 2)].map(|v| v as f32)
}

fn dump(args: &[String]) -> anyhow::Result<()> {
    anyhow::ensure!(args.len() == 2, "tree_v2 dump <v2 tree> <out.f32>");
    let tree = Tree::parse(&std::fs::read(&args[0])?)?;
    let g = &tree.splats;
    let mut out = Vec::with_capacity(tree.len() * 48 + tree.nodes.len() * 32);
    let mut owner = vec![0u32; tree.len()];
    for (b, n) in tree.nodes.iter().enumerate() {
        owner[n.start as usize..(n.start + n.count) as usize].fill(b as u32);
    }
    let byte = |v: f32| (v * 255.0).round().clamp(0.0, 255.0) as u32;
    for i in 0..tree.len() {
        let c = g.colors[i];
        let alpha = byte(orbiters_reconstruct::gaussians::sigmoid(g.opacity_logits[i]));
        let cov = covariance(g, i);
        let values = [
            g.means[i][0],
            g.means[i][1],
            g.means[i][2],
            cov[0],
            cov[1],
            cov[2],
            cov[3],
            cov[4],
            cov[5],
            (byte(c[0]) | byte(c[1]) << 8 | byte(c[2]) << 16) as f32,
            alpha as f32 / 255.0,
            owner[i] as f32,
        ];
        for v in values {
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
    for n in &tree.nodes {
        for v in [n.center[0], n.center[1], n.center[2], n.radius, n.error, n.parent.map_or(-1.0, |p| p as f32), n.start as f32, n.count as f32] {
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
    std::fs::write(&args[1], &out)?;
    println!("{{\"splats\":{},\"boxes\":{}}}", tree.len(), tree.nodes.len());
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("convert") => convert(&args[1..]),
        Some("dump") => dump(&args[1..]),
        _ => anyhow::bail!("tree_v2 convert <v1 tree> <v2 out> [--coarse <out>] | tree_v2 dump <v2 tree> <out.f32>"),
    }
}
