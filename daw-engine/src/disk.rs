//! Disk-space helpers for recording.

use std::path::Path;

/// Free bytes available to the current process on the filesystem containing
/// `path`. Returns `None` on platforms without `statvfs` or when the call
/// fails.
#[cfg(unix)]
pub fn available_bytes(path: &Path) -> Option<u64> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let path = CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut stats: libc::statvfs = unsafe { std::mem::zeroed() };
    let result = unsafe { libc::statvfs(path.as_ptr(), &mut stats) };
    if result == 0 {
        Some(
            (stats.f_bavail as u64).saturating_mul(stats.f_frsize as u64),
        )
    } else {
        None
    }
}

#[cfg(not(unix))]
pub fn available_bytes(_path: &Path) -> Option<u64> {
    None
}

/// Seconds of recording that fit in `bytes` at the given format.
pub fn recording_seconds_for_bytes(
    bytes: u64,
    sample_rate: u32,
    channels: u16,
    bits_per_sample: u16,
) -> f64 {
    let sample_rate = sample_rate.max(1) as f64;
    let channels = channels.max(1) as f64;
    let bytes_per_sample = (bits_per_sample.max(1) as f64 / 8.0).max(1.0);
    let bytes_per_second = sample_rate * channels * bytes_per_sample;
    bytes as f64 / bytes_per_second
}

/// Seconds of recording available on the filesystem containing `path`.
pub fn recording_seconds_available(
    path: &Path,
    sample_rate: u32,
    channels: u16,
    bits_per_sample: u16,
) -> Option<f64> {
    available_bytes(path).map(|bytes| {
        recording_seconds_for_bytes(bytes, sample_rate, channels, bits_per_sample)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recording_time_scales_with_format_and_space() {
        // 1 GiB at 48 kHz stereo 32-bit float.
        let seconds = recording_seconds_for_bytes(1 << 30, 48_000, 2, 32);
        assert!((seconds - 2796.2).abs() < 0.5);

        // 16-bit uses half the space, so the same bytes last twice as long.
        let seconds_16 = recording_seconds_for_bytes(1 << 30, 48_000, 2, 16);
        assert!((seconds_16 - seconds * 2.0).abs() < 1.0);

        // Mono halves the space per second.
        let seconds_mono = recording_seconds_for_bytes(1 << 30, 48_000, 1, 32);
        assert!((seconds_mono - seconds * 2.0).abs() < 1.0);
    }

    #[test]
    fn available_bytes_finds_the_local_filesystem() {
        // The current directory always exists, so this should succeed on Unix.
        assert!(available_bytes(Path::new(".")).is_some());
    }
}
