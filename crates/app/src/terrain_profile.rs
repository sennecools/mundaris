//! Bounded loading for the opt-in Moon height-profile fixture.
//!
//! The profile is an experiment input. It does not add a default asset lookup
//! or change the source used by ordinary Moon routes.

use std::{fs::File, io::Read, path::Path};

use anyhow::{Context, ensure};
use mundaris_world::terrain::TerrainHeightProfile;

pub const MOON_PROFILE_WIDTH: u32 = 2048;
pub const MOON_PROFILE_HEIGHT: u32 = 2048;
pub const MOON_PROFILE_BYTES: u64 = MOON_PROFILE_WIDTH as u64 * MOON_PROFILE_HEIGHT as u64 * 2;

/// Load a square big-endian u16 raw profile, inferring its side from the exact
/// number of samples. The inferred side must be an integer in 2..=2048.
pub fn load_height_profile(path: &Path) -> anyhow::Result<TerrainHeightProfile> {
    let file =
        File::open(path).with_context(|| format!("opening height profile {}", path.display()))?;
    let metadata_len = file
        .metadata()
        .with_context(|| format!("reading height profile metadata {}", path.display()))?
        .len();
    ensure!(
        metadata_len <= MOON_PROFILE_BYTES,
        "height profile is oversized: {metadata_len} bytes; maximum is {MOON_PROFILE_BYTES}"
    );
    ensure!(
        metadata_len % 2 == 0,
        "height profile byte length must be even"
    );
    let side = infer_square_side(metadata_len / 2)?;
    load_open_profile(file, path, side, side, metadata_len)
}

/// Load a big-endian u16 profile with explicitly supplied dimensions. This is
/// the entry point for scale-specific and rectangular fixtures.
pub fn load_height_profile_dimensions(
    path: &Path,
    width: u32,
    height: u32,
) -> anyhow::Result<TerrainHeightProfile> {
    let expected_len = expected_profile_bytes(width, height)?;
    let file =
        File::open(path).with_context(|| format!("opening height profile {}", path.display()))?;
    let metadata_len = file
        .metadata()
        .with_context(|| format!("reading height profile metadata {}", path.display()))?
        .len();
    ensure!(
        metadata_len <= MOON_PROFILE_BYTES,
        "height profile is oversized: {metadata_len} bytes; maximum is {MOON_PROFILE_BYTES}"
    );
    ensure!(
        metadata_len == expected_len,
        "height profile has {metadata_len} bytes; expected exactly {expected_len} for {width}x{height}"
    );
    load_open_profile(file, path, width, height, expected_len)
}

fn expected_profile_bytes(width: u32, height: u32) -> anyhow::Result<u64> {
    ensure!(
        (2..=MOON_PROFILE_WIDTH).contains(&width),
        "height profile width must be in 2..={MOON_PROFILE_WIDTH}"
    );
    ensure!(
        (2..=MOON_PROFILE_HEIGHT).contains(&height),
        "height profile height must be in 2..={MOON_PROFILE_HEIGHT}"
    );
    let samples = u64::from(width)
        .checked_mul(u64::from(height))
        .context("height profile dimensions overflow")?;
    samples
        .checked_mul(2)
        .context("height profile byte length overflow")
}

fn infer_square_side(sample_count: u64) -> anyhow::Result<u32> {
    for side in 2..=MOON_PROFILE_WIDTH {
        let square = u64::from(side) * u64::from(side);
        if square == sample_count {
            return Ok(side);
        }
        if square > sample_count {
            break;
        }
    }
    anyhow::bail!(
        "height profile sample count {sample_count} is not a square with side in 2..={MOON_PROFILE_WIDTH}"
    )
}

fn load_open_profile(
    file: File,
    path: &Path,
    width: u32,
    height: u32,
    expected_len: u64,
) -> anyhow::Result<TerrainHeightProfile> {
    let mut bounded = file.take(expected_len + 1);
    let mut bytes = Vec::with_capacity(expected_len as usize);
    bounded
        .read_to_end(&mut bytes)
        .with_context(|| format!("reading height profile {}", path.display()))?;
    ensure!(
        bytes.len() as u64 == expected_len,
        "height profile changed while being read: got {} bytes; expected {expected_len}",
        bytes.len(),
    );

    // External raw fixtures use big endian. World content uses canonical little-
    // endian input bytes, so equivalent samples have one stable content identity.
    canonicalize_big_endian(&mut bytes);
    TerrainHeightProfile::from_u16_le(width, height, &bytes)
        .with_context(|| format!("validating {width}x{height} big-endian height profile"))
}

fn canonicalize_big_endian(bytes: &mut [u8]) {
    for pair in bytes.as_chunks_mut::<2>().0 {
        pair.swap(0, 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        sync::atomic::{AtomicU64, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    static NEXT_FILE: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn external_big_endian_values_convert_to_canonical_world_bytes() {
        let mut bytes = [0x12, 0x34, 0xab, 0xcd];
        canonicalize_big_endian(&mut bytes);
        assert_eq!(bytes, [0x34, 0x12, 0xcd, 0xab]);
        assert_eq!(u16::from_le_bytes([bytes[0], bytes[1]]), 0x1234);
    }

    struct TempFile(std::path::PathBuf);

    impl TempFile {
        fn with_bytes(bytes: &[u8]) -> Self {
            let unique = NEXT_FILE.fetch_add(1, Ordering::Relaxed);
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock is after Unix epoch")
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "mundaris-height-profile-{}-{stamp}-{unique}.r16",
                std::process::id()
            ));
            fs::write(&path, bytes).expect("write small temporary fixture");
            Self(path)
        }
    }

    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }

    #[test]
    fn infers_small_square_dimensions_and_converts_big_endian_samples() {
        let external = [0x12, 0x34, 0xab, 0xcd, 0x00, 0x01, 0xff, 0xfe];
        let file = TempFile::with_bytes(&external);
        let loaded = load_height_profile(&file.0).expect("2x2 square fixture should load");
        let canonical = [0x34, 0x12, 0xcd, 0xab, 0x01, 0x00, 0xfe, 0xff];
        let expected = TerrainHeightProfile::from_u16_le(2, 2, &canonical).unwrap();
        assert_eq!(loaded, expected);
    }

    #[test]
    fn rejects_even_byte_file_with_nonsquare_sample_count() {
        let file = TempFile::with_bytes(&[0; 12]); // Six u16 samples.
        let error = load_height_profile(&file.0).expect_err("six samples are not a square");
        assert!(error.to_string().contains("not a square"));
    }

    #[test]
    fn rejects_odd_byte_file() {
        let file = TempFile::with_bytes(&[0; 7]);
        let error = load_height_profile(&file.0).expect_err("odd byte length must fail");
        assert!(error.to_string().contains("must be even"));
    }

    #[test]
    fn loads_explicit_rectangular_dimensions() {
        let external = [
            0x00, 0x01, 0x10, 0x20, 0x30, 0x40, 0xab, 0xcd, 0xfe, 0xdc, 0x55, 0xaa,
        ];
        let file = TempFile::with_bytes(&external);
        let loaded = load_height_profile_dimensions(&file.0, 3, 2)
            .expect("3x2 rectangular fixture should load");
        let canonical = [
            0x01, 0x00, 0x20, 0x10, 0x40, 0x30, 0xcd, 0xab, 0xdc, 0xfe, 0xaa, 0x55,
        ];
        let expected = TerrainHeightProfile::from_u16_le(3, 2, &canonical).unwrap();
        assert_eq!(loaded, expected);
    }

    #[test]
    fn rejects_explicit_dimensions_outside_supported_bounds() {
        let file = TempFile::with_bytes(&[]);
        let width_error = load_height_profile_dimensions(&file.0, 2049, 2)
            .expect_err("width above maximum must fail");
        assert!(width_error.to_string().contains("width must be"));
        let height_error = load_height_profile_dimensions(&file.0, 2, 2049)
            .expect_err("height above maximum must fail");
        assert!(height_error.to_string().contains("height must be"));
        let small_error = load_height_profile_dimensions(&file.0, 1, 2)
            .expect_err("width below minimum must fail");
        assert!(small_error.to_string().contains("width must be"));
    }

    #[test]
    fn rejects_oversized_metadata_before_reading_profile_bytes() {
        let file = TempFile::with_bytes(&[]);
        fs::OpenOptions::new()
            .write(true)
            .open(&file.0)
            .expect("open temporary sparse fixture")
            .set_len(MOON_PROFILE_BYTES + 1)
            .expect("extend sparse fixture length");
        let error = load_height_profile(&file.0).expect_err("oversized fixture must fail");
        assert!(error.to_string().contains("oversized"));
    }

    #[test]
    fn reports_missing_fixture_path() {
        let unique = NEXT_FILE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "mundaris-height-profile-missing-{}-{unique}.r16",
            std::process::id()
        ));
        let error = load_height_profile(&path).expect_err("missing fixture must fail");
        assert!(error.to_string().contains("opening height profile"));
    }
}
