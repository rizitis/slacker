//! Checks on the files under `data/` that get installed with the binary.

use std::path::Path;

const SIZES: [u32; 9] = [16, 22, 24, 32, 48, 64, 128, 256, 512];

fn data() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("data")
}

/// Width and height from a PNG's IHDR chunk.
fn png_size(bytes: &[u8]) -> (u32, u32) {
    assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n", "not a PNG");
    assert_eq!(&bytes[12..16], b"IHDR");
    let w = u32::from_be_bytes(bytes[16..20].try_into().unwrap());
    let h = u32::from_be_bytes(bytes[20..24].try_into().unwrap());
    (w, h)
}

#[test]
fn every_icon_size_is_present_and_square() {
    for s in SIZES {
        let p = data()
            .join(format!("icons/hicolor/{s}x{s}/apps"))
            .join(format!("{}.png", crate::APP_ID));
        let bytes = std::fs::read(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
        assert_eq!(png_size(&bytes), (s, s), "{}", p.display());
    }
    let svg = data()
        .join("icons/hicolor/scalable/apps")
        .join(format!("{}.svg", crate::APP_ID));
    let text = std::fs::read_to_string(&svg).unwrap();
    assert!(text.contains(r#"viewBox="0 0 512 512""#), "scalable icon must be square");
}

#[test]
fn desktop_file_matches_the_app_id() {
    let p = data().join(format!("{}.desktop", crate::APP_ID));
    let text = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    assert!(text.lines().any(|l| l == format!("Icon={}", crate::APP_ID)));
    assert!(text.lines().any(|l| l == "Exec=slacker-flatpak"));
}
