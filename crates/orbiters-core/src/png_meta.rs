//! A copy of a PNG without what it says about who took it (PRIV-8): VRChat writes the taker's name and VRChat id in its
//! XMP (`iTXt XML:com.adobe.xmp`), VRCX adds the instance and everyone present with their ids (`iTXt Description`), and
//! other tools may add comments, EXIF or GPS. What leaves for Orbiters is the picture; the world and the date go beside
//! it, in the request. The file on this computer is never changed: what the gallery and the visited worlds read in it
//! stays there.
//!
//! The copy keeps the chunks that make the picture (an allowlist: header, palette, transparency, colour, physical size,
//! animation, the image data in order) byte for byte, so its pixels are the original's exactly; it leaves out every
//! text, EXIF and time chunk, any other ancillary chunk, and anything after the end. A critical chunk it does not know
//! (one a decoder must understand) is refused: the picture is not sent rather than sent altered or as it was.

use std::io::{Read, Write};
use std::path::Path;

const SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";

/// The chunks a copy keeps: what draws the picture.
const KEPT: [&[u8; 4]; 16] = [
    b"IHDR", b"PLTE", b"IDAT", b"IEND", b"tRNS", b"cHRM", b"gAMA", b"iCCP", b"sBIT", b"sRGB", b"cICP", b"mDCv", b"cLLi", b"acTL", b"fcTL", b"fdAT",
];

/// Writes `src`'s PNG to `out` without its metadata chunks; returns the bytes written.
pub fn copy_without_metadata(src: &mut impl Read, out: &mut impl Write) -> std::io::Result<u64> {
    let bad = |what: &str| std::io::Error::new(std::io::ErrorKind::InvalidData, what.to_string());
    let mut signature = [0u8; 8];
    src.read_exact(&mut signature)?;
    if &signature != SIGNATURE {
        return Err(bad(orbiters_i18n::tr("not a PNG")));
    }
    out.write_all(SIGNATURE)?;
    let mut written = 8u64;
    let mut buffer = vec![0u8; 64 << 10];
    loop {
        let mut head = [0u8; 8];
        src.read_exact(&mut head)?;
        let length = u32::from_be_bytes([head[0], head[1], head[2], head[3]]) as u64;
        let kind: [u8; 4] = [head[4], head[5], head[6], head[7]];
        if length > (1u64 << 31) || !kind.iter().all(u8::is_ascii_alphabetic) {
            return Err(bad(orbiters_i18n::tr("a damaged PNG chunk")));
        }
        let keep = KEPT.iter().any(|k| **k == kind);
        // Bit 5 of the first letter: lowercase is ancillary (may be left out), uppercase critical (must be understood).
        if !keep && kind[0].is_ascii_uppercase() {
            return Err(bad(orbiters_i18n::tr("a PNG chunk this copy cannot keep")));
        }
        if keep {
            out.write_all(&head)?;
        }
        // The data and its CRC, copied or passed over.
        let mut left = length + 4;
        while left > 0 {
            let n = left.min(buffer.len() as u64) as usize;
            src.read_exact(&mut buffer[..n])?;
            if keep {
                out.write_all(&buffer[..n])?;
            }
            left -= n as u64;
        }
        if keep {
            written += 8 + length + 4;
        }
        if &kind == b"IEND" {
            // Whatever follows the end is not the picture's.
            return Ok(written);
        }
    }
}

/// [`copy_without_metadata`] of the file at `src` into a new file at `dst` (flushed); its length.
pub fn write_without_metadata(src: &Path, dst: &Path) -> std::io::Result<u64> {
    let mut input = std::io::BufReader::new(std::fs::File::open(src)?);
    let file = std::fs::File::create(dst)?;
    let mut output = std::io::BufWriter::new(file);
    let written = copy_without_metadata(&mut input, &mut output);
    let written = written.and_then(|n| {
        output.flush()?;
        Ok(n)
    });
    if written.is_err() {
        drop(output);
        let _ = std::fs::remove_file(dst);
    }
    written
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut c = (data.len() as u32).to_be_bytes().to_vec();
        c.extend_from_slice(kind);
        c.extend_from_slice(data);
        let mut crc = flate2::Crc::new();
        crc.update(kind);
        crc.update(data);
        c.extend_from_slice(&crc.sum().to_be_bytes());
        c
    }

    fn itxt(keyword: &str, text: &str) -> Vec<u8> {
        let mut d = keyword.as_bytes().to_vec();
        d.extend_from_slice(&[0, 0, 0, 0, 0]);
        d.extend_from_slice(text.as_bytes());
        chunk(b"iTXt", &d)
    }

    /// A 2 x 1 RGBA picture with what VRChat and VRCX write in theirs, a tEXt, an eXIf, a tIME, and bytes after its end.
    fn shot() -> (Vec<u8>, Vec<u8>) {
        let pixels = [0u8, 10, 20, 30, 255, 40, 50, 60, 128];
        let idat = {
            let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
            z.write_all(&pixels).unwrap();
            z.finish().unwrap()
        };
        let mut ihdr = 2u32.to_be_bytes().to_vec();
        ihdr.extend_from_slice(&1u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
        let mut png = SIGNATURE.to_vec();
        png.extend(chunk(b"IHDR", &ihdr));
        png.extend(chunk(b"sRGB", &[0]));
        png.extend(itxt(
            "XML:com.adobe.xmp",
            "<x:xmpmeta><rdf:Description xmp:Author=\"Nova\" vrc:AuthorID=\"usr_00000000-0000-4000-8000-000000000001\" vrc:WorldID=\"wrld_x\"/></x:xmpmeta>",
        ));
        png.extend(itxt(
            "Description",
            r#"{"author":{"displayName":"Nova","id":"usr_00000000-0000-4000-8000-000000000001"},"world":{"id":"wrld_x","instanceId":"1~private(usr_00000000-0000-4000-8000-000000000001)"},"players":[{"displayName":"Kite","id":"usr_00000000-0000-4000-8000-000000000002"}]}"#,
        ));
        png.extend(chunk(b"tEXt", b"Comment\0taken by Nova"));
        png.extend(chunk(b"eXIf", b"MM\0*gps"));
        png.extend(chunk(b"tIME", &[7, 234, 11, 19, 1, 2, 3]));
        let kept_idat = chunk(b"IDAT", &idat);
        png.extend(&kept_idat);
        png.extend(chunk(b"IEND", &[]));
        png.extend_from_slice(b"trailing usr_00000000-0000-4000-8000-000000000003");
        (png, pixels.to_vec())
    }

    #[test]
    fn the_copy_has_the_picture_and_nothing_of_who_took_it() {
        let (original, pixels) = shot();
        let mut out = Vec::new();
        let n = copy_without_metadata(&mut original.as_slice(), &mut out).unwrap();
        assert_eq!(n, out.len() as u64);
        let text = String::from_utf8_lossy(&out);
        for gone in ["usr_", "Nova", "Kite", "wrld_x", "private", "Comment", "gps", "tIME", "trailing", "iTXt", "tEXt", "eXIf"] {
            assert!(!text.contains(gone), "{gone} left in the copy");
        }
        // The picture itself, byte for byte: the same chunks that draw it, in order.
        let kinds = |png: &[u8]| {
            let mut at = 8;
            let mut kinds = Vec::new();
            while at + 8 <= png.len() {
                let len = u32::from_be_bytes(png[at..at + 4].try_into().unwrap()) as usize;
                kinds.push((png[at + 4..at + 8].to_vec(), png[at + 8..at + 8 + len].to_vec()));
                at += 12 + len;
                if &kinds.last().unwrap().0[..] == b"IEND" {
                    break;
                }
            }
            kinds
        };
        let kept: Vec<_> = kinds(&original).into_iter().filter(|(k, _)| [&b"IHDR"[..], b"sRGB", b"IDAT", b"IEND"].contains(&&k[..])).collect();
        assert_eq!(kinds(&out), kept);
        let idat = &kinds(&out).iter().find(|(k, _)| k == b"IDAT").unwrap().1.clone();
        let mut inflated = Vec::new();
        flate2::read::ZlibDecoder::new(idat.as_slice()).read_to_end(&mut inflated).unwrap();
        assert_eq!(inflated, pixels, "pixels untouched");
        // The file itself is not changed: a copy is written beside.
        let dir = std::env::temp_dir().join(format!("orbiters-png-meta-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let (src, dst) = (dir.join("shot.png"), dir.join("clean.png"));
        std::fs::write(&src, &original).unwrap();
        assert_eq!(write_without_metadata(&src, &dst).unwrap(), out.len() as u64);
        assert_eq!(std::fs::read(&dst).unwrap(), out);
        assert_eq!(std::fs::read(&src).unwrap(), original);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn what_cannot_be_copied_safely_is_refused() {
        assert!(copy_without_metadata(&mut &b"GIF89a.."[..], &mut Vec::new()).is_err(), "not a PNG");
        let (mut png, _) = shot();
        let end = png.len() - "trailing usr_00000000-0000-4000-8000-000000000003".len() - 12;
        png.splice(end..end, chunk(b"QUUX", b"must be understood"));
        assert!(copy_without_metadata(&mut png.as_slice(), &mut Vec::new()).is_err(), "an unknown critical chunk");
        let (png, _) = shot();
        assert!(copy_without_metadata(&mut &png[..40], &mut Vec::new()).is_err(), "cut short");
    }
}
