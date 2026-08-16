//! #379/#380 end to end: the issue's own three files, through the real
//! `sniff()` entry point rather than through `sniff_bytes` in a unit test.
//!
//! `BM` and `GIF8` were magic signatures made only of printable ASCII, believed
//! without checking anything behind them, so `cars.csv` opening `BMW,model,year`
//! and a note opening `GIF89a is the header format…` classified `Family::Binary`
//! and `scan_file` junked them as "binary content (bmp)" / "(gif)" — never
//! indexed, with a reason that was not true. `id3-notes.txt` is the control: it
//! was already correct on rc.16 and must stay correct.
//!
//! The qualifiers themselves are unit-tested in `sniff::printable_magic_tests`.
//! What this file adds is the path the user actually takes: bytes on disk, read
//! by `read_prefix`, classified by `sniff(&Path)`.

use std::path::Path;
use xerj_autoindex::sniff::{sniff, Family};

fn write(dir: &Path, name: &str, body: &str) -> std::path::PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, body).unwrap();
    p
}

#[test]
fn the_issue_folder_indexes_every_file() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    let cars = write(
        root,
        "cars.csv",
        "BMW,model,year\nX5,SUV,2024\n330i,sedan,2023\nM3,coupe,2022\niX,SUV,2025\n",
    );
    let gif = write(
        root,
        "gif-notes.txt",
        &"GIF89a is the header format used by the GIF image standard. ".repeat(30),
    );
    let id3 = write(
        root,
        "id3-notes.txt",
        &"ID3 tags are metadata containers embedded in MP3 files. ".repeat(30),
    );

    // rc.16: binary/bmp, binary/gif, txt-prose — 3 files in, 1 indexed.
    for (path, want) in [
        (&cars, Family::Csv),
        (&gif, Family::TxtProse),
        (&id3, Family::TxtProse),
    ] {
        let sn = sniff(path).unwrap();
        assert_eq!(
            sn.family,
            want,
            "{}: classified {} ({:?}), which `scan_file` turns into junk",
            path.file_name().unwrap().to_string_lossy(),
            sn.family.as_str(),
            sn.binary_kind
        );
    }
}

/// The other half of the fix: a real bitmap on disk must still be caught by its
/// magic bytes, before any text heuristic. Without this the fix would just move
/// the damage — an image reaching the text path is what sections a multi-MB
/// file into thousands of prose records.
#[test]
fn a_real_bitmap_on_disk_is_still_binary() {
    let dir = tempfile::tempdir().unwrap();

    // BITMAPFILEHEADER (14) + BITMAPINFOHEADER (40), 2x2 24bpp.
    let mut bmp: Vec<u8> = b"BM".to_vec();
    bmp.extend_from_slice(&70u32.to_le_bytes());
    bmp.extend_from_slice(&[0, 0, 0, 0]);
    bmp.extend_from_slice(&54u32.to_le_bytes());
    bmp.extend_from_slice(&40u32.to_le_bytes());
    bmp.extend_from_slice(&2i32.to_le_bytes());
    bmp.extend_from_slice(&2i32.to_le_bytes());
    bmp.extend_from_slice(&[1, 0, 24, 0]);
    bmp.extend(std::iter::repeat_n(0u8, 24));
    // A printable tail that would otherwise pass the prose heuristics.
    bmp.extend(b"lorem ipsum dolor sit amet, consectetur adipiscing elit. ".repeat(20));

    let p = dir.path().join("logo.bmp");
    std::fs::write(&p, &bmp).unwrap();
    let sn = sniff(&p).unwrap();
    assert_eq!(sn.family, Family::Binary);
    assert_eq!(sn.binary_kind.as_deref(), Some("bmp"));
}

/// The comment arm on the path that actually loses data: a file on disk whose
/// comment is longer than the 8 KiB `read_prefix` buffer, so the sub-block
/// chain cannot be walked to its end and the terminator is never seen.
///
/// The unit fixtures in `sniff::printable_magic_tests` truncate to 8192 by
/// hand; these are real files of 20 KB and 60 KB, cut by `read_prefix` itself.
/// The chain shapes are the ones the ground-truth corpus found on disk —
/// giflib's streaming API writes one sub-block per `EGifPutExtensionBlock` call
/// at the caller's length, so 64-byte and 100-byte chains are ordinary output,
/// and the "every size is 0xFF" rule that round 3 shipped refuses them: 43 of
/// the corpus's 92 outrunning images, 37 of them into `TxtProse`.
///
/// The negative in the same shape is the file below it: prose only has to land
/// `!` and a windows-1252 `0xFE`, and it must NOT be junked.
#[test]
fn a_long_commented_gif_on_disk_survives_the_prefix_cut() {
    let dir = tempfile::tempdir().unwrap();

    fn streamed(sub_block: u8, comment_len: usize) -> Vec<u8> {
        let mut g: Vec<u8> = b"GIF89a".to_vec();
        g.extend_from_slice(&48u16.to_le_bytes());
        g.extend_from_slice(&48u16.to_le_bytes());
        g.push(0x80); // GCT present, 2 entries
        g.push(0);
        g.push(0);
        g.extend_from_slice(&[0, 0, 0, 0xff, 0xff, 0xff]);
        g.extend_from_slice(&[0x21, 0xfe]);
        let mut left = comment_len;
        while left > 0 {
            let take = left.min(usize::from(sub_block));
            g.push(take as u8);
            g.extend(std::iter::repeat_n(b'D', take));
            left -= take;
        }
        g.push(0x00);
        g.extend_from_slice(&[0x2c, 0, 0, 0, 0, 48, 0, 48, 0, 0, 0x02]);
        g
    }

    for (name, sub_block, len) in [
        ("stream64.gif", 64u8, 20_000usize),
        ("stream100.gif", 100, 60_000),
        ("stream255.gif", 255, 60_000),
    ] {
        let p = dir.path().join(name);
        let bytes = streamed(sub_block, len);
        assert!(bytes.len() > 8192, "{name}: fixture must outrun the prefix");
        std::fs::write(&p, &bytes).unwrap();
        let sn = sniff(&p).unwrap();
        assert_eq!(
            (sn.family, sn.binary_kind.as_deref()),
            (Family::Binary, Some("gif")),
            "{name}: a real GIF with a {sub_block}-byte comment chain classified \
             {}/{:?} — `scan_file` hands that to the prose extractor",
            sn.family.as_str(),
            sn.binary_kind
        );
    }

    // The same shape, in prose: `!` at 13, a windows-1252 0xFE at 14, and a
    // chain of printable "sizes" that never terminates inside the prefix.
    let mut notes: Vec<u8> = b"GIF89a fichie!".to_vec();
    notes.push(0xfe);
    for _ in 0..400 {
        notes.extend_from_slice("Le format GIF reste partout sur le web. ".as_bytes());
    }
    let np = dir.path().join("notes.txt");
    std::fs::write(&np, &notes).unwrap();
    let sn = sniff(&np).unwrap();
    assert_eq!(
        sn.family,
        Family::TxtProse,
        "prose was junked as {:?} through the comment arm (#379)",
        sn.binary_kind
    );
}

/// An actual GIF, checked into this repository, read off disk.
///
/// Every other fixture in this file and in `sniff::printable_magic_tests` is
/// bytes an author typed, which is exactly how round 1 of #379 shipped a
/// qualifier that refused 4 of the 147 distinct real GIFs on the build machine
/// — `docs/media/demo.gif` among them, because its Pixel Aspect Ratio byte is
/// 49 (`(49 + 15) / 64 = 1.0`, the spec's encoding of square pixels) and the
/// qualifier demanded 0. Hand-written headers all set it to 0, so no test saw
/// it. This one does: 3.27 MB, 720x450, animated, NETSCAPE2.0 loop extension
/// behind a 768-byte global colour table.
///
/// Measured with the round-1 qualifier in place, this file classifies
/// `binary`/`unknown`: its first 8 KiB is 11.45% control characters, so the
/// heuristic behind the magic table caught it with 1.45 points to spare. That
/// margin is the whole reason the assertion below names `Some("gif")` and not
/// just `Family::Binary` — two other GIFs in the same 147-file sweep decode to
/// 9.39% and 9.78% and would have been sectioned into prose records instead.
#[test]
fn the_repositorys_own_demo_gif_is_binary_by_magic() {
    // engine/crates/xerj-autoindex -> repository root.
    let gif = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../docs/media/demo.gif")
        .canonicalize()
        .expect(
            "docs/media/demo.gif is a tracked file and this test exists to read \
             it; if it moved, point this at another real GIF rather than \
             deleting the only non-synthetic fixture",
        );

    // Guard against silently testing a placeholder: assert the shape that made
    // this file the counter-example before asserting the classification.
    let head = std::fs::read(&gif).unwrap();
    assert!(head.starts_with(b"GIF89a"), "fixture is no longer a GIF");
    assert_ne!(
        head[12], 0,
        "demo.gif no longer declares a pixel aspect ratio, so it no longer \
         exercises the round-1 regression — find another real GIF that does"
    );

    let sn = sniff(&gif).unwrap();
    assert_eq!(
        (sn.family, sn.binary_kind.as_deref()),
        (Family::Binary, Some("gif")),
        "a real 3.27 MB GIF classified {}/{:?} — `scan_file` hands that to the \
         text extractor",
        sn.family.as_str(),
        sn.binary_kind
    );
}
