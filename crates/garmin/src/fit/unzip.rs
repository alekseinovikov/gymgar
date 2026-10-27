//! Taking the `.FIT` out of the ZIP the Personal API sends (`<activity id>_ACTIVITY.fit`).
//!
//! The archive is hostile input: it may be damaged, hold several files or none, or be a ZIP bomb. Limits apply to the archive and
//! to what is actually unpacked, never to the sizes the archive declares about itself. No file is ever written to disk.

use std::io::{Cursor, Read};

use zip::ZipArchive;

use super::FitError;

/// Largest archive accepted. A real strength activity is tens of kilobytes; the download is capped at the same 32 MiB.
const MAX_ZIP_BYTES: usize = 32 * 1024 * 1024;
/// Largest `.FIT` accepted once unpacked. Days of second-by-second records stay well below this.
const MAX_FIT_BYTES: u64 = 64 * 1024 * 1024;

/// The bytes of the one `.fit` entry (extension in any case) of `zip`. [`FitError::TooLarge`] past the limits, [`FitError::Zip`] for
/// anything else: not a ZIP, damaged, no `.fit` or more than one.
pub fn unzip_activity(zip: &[u8]) -> Result<Vec<u8>, FitError> {
    unzip_within(zip, MAX_ZIP_BYTES, MAX_FIT_BYTES)
}

/// [`unzip_activity`] with the limits as parameters, so tests can reach them without megabytes of input.
fn unzip_within(zip: &[u8], max_zip: usize, max_fit: u64) -> Result<Vec<u8>, FitError> {
    if zip.len() > max_zip {
        return Err(FitError::TooLarge);
    }
    let mut archive = ZipArchive::new(Cursor::new(zip)).map_err(|error| FitError::Zip(error.to_string()))?;
    let mut fit_entries = (0..archive.len()).filter(|&index| archive.name_for_index(index).and_then(Result::ok).is_some_and(|name| is_fit(&name)));
    let index = match (fit_entries.next(), fit_entries.next()) {
        (Some(index), None) => index,
        (None, _) => return Err(FitError::Zip("no .fit file in the archive".to_owned())),
        (Some(_), Some(_)) => return Err(FitError::Zip("more than one .fit file in the archive".to_owned())),
    };
    let entry = archive.by_index(index).map_err(|error| FitError::Zip(error.to_string()))?;
    // Counting what comes out, not the size the entry declares: one byte past the limit is enough to know it is too large.
    let mut fit = Vec::new();
    entry.take(max_fit.saturating_add(1)).read_to_end(&mut fit).map_err(|error| FitError::Zip(error.to_string()))?;
    if u64::try_from(fit.len()).map_or(true, |len| len > max_fit) {
        return Err(FitError::TooLarge);
    }
    Ok(fit)
}

fn is_fit(name: &str) -> bool {
    name.to_ascii_lowercase().ends_with(".fit")
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use chrono::{TimeZone, Utc};
    use zip::write::SimpleFileOptions;
    use zip::{CompressionMethod, ZipWriter};

    use super::*;
    use crate::fit::testing::{FitWriter, zip_single};

    fn fit_file() -> Vec<u8> {
        let mut writer = FitWriter::new(Utc.with_ymd_and_hms(2026, 10, 20, 17, 5, 9).unwrap());
        writer.workout("FB2");
        writer.finish()
    }

    /// A ZIP with these entries, in this order.
    fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
        for (name, bytes) in entries {
            zip.start_file(*name, SimpleFileOptions::default().compression_method(CompressionMethod::Deflated)).unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    fn zip_error(result: Result<Vec<u8>, FitError>) -> String {
        match result {
            Err(FitError::Zip(reason)) => reason,
            other => panic!("expected FitError::Zip, got {other:?}"),
        }
    }

    #[test]
    fn unzip_round_trip() {
        let fit = fit_file();
        assert_eq!(unzip_activity(&zip_single("1_ACTIVITY.fit", &fit)), Ok(fit.clone()));
        // The extension in any case, and other entries next to the one `.fit` do not matter.
        assert_eq!(unzip_activity(&zip_of(&[("notes.txt", b"hello"), ("2_ACTIVITY.FIT", &fit)])), Ok(fit));
    }

    #[test]
    fn unzip_rejects() {
        let fit = fit_file();
        zip_error(unzip_activity(b"this is not a zip archive"));
        zip_error(unzip_activity(&[]));
        zip_error(unzip_activity(&zip_of(&[])));
        zip_error(unzip_activity(&zip_of(&[("a.fit", &fit), ("b.fit", &fit)])));
        zip_error(unzip_activity(&zip_single("notes.txt", b"hello")));
        // Damaged: the deflate stream of the entry is overwritten.
        let mut damaged = zip_single("1_ACTIVITY.fit", &fit);
        for byte in damaged.iter_mut().skip(40).take(20) {
            *byte ^= 0x5A;
        }
        assert!(unzip_activity(&damaged).is_err());
    }

    #[test]
    fn unzip_counts_what_it_unpacks() {
        // A ZIP bomb in miniature: a megabyte of zeros packs into about a kilobyte.
        let bomb = zip_single("1_ACTIVITY.fit", &vec![0; 1 << 20]);
        assert!(bomb.len() < 16 * 1024, "{}", bomb.len());
        assert_eq!(unzip_within(&bomb, bomb.len(), 64 * 1024), Err(FitError::TooLarge));
        // Exactly at the limit is still fine.
        assert_eq!(unzip_within(&bomb, bomb.len(), 1 << 20).map(|fit| fit.len()), Ok(1 << 20));
        // The archive itself is checked before anything is read.
        assert_eq!(unzip_within(&bomb, bomb.len() - 1, 1 << 20), Err(FitError::TooLarge));
    }

    #[test]
    fn real_limits_are_32_and_64_mebibytes() {
        assert_eq!(MAX_ZIP_BYTES, 32 * 1024 * 1024);
        assert_eq!(MAX_FIT_BYTES, 64 * 1024 * 1024);
    }
}
