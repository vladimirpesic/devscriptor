//! Memory-mapped file reader for efficient large file handling.
//!
//! Files larger than [`MmapReader::mmap_threshold`] are mapped read-only via
//! `mmap(2)` and exposed as a borrowed `&str` (zero-copy). UTF-8 validation
//! happens once at open time. Small files use ordinary `std::fs::read_to_string`
//! since `mmap` overhead dominates for sub-page reads.
//!
//! # Notes
//!
//! - The mapping is **not** locked against external writers. Concurrent writes
//!   from another process can produce torn reads or invalid UTF-8 mid-mapping,
//!   a caveat of using `mmap` for arbitrary files; callers that care should not
//!   rely on this type for shared / writable files. (See
//!   [`crate::utils::security::FileChangeDetector`] for a best-effort change
//!   detector based on size + mtime.) UTF-8 is validated once, at open time:
//!   validating at every `as_str()` would defeat the zero-copy read and still
//!   be racy.

use std::fs::File;
#[cfg(unix)]
use std::os::fd::AsRawFd;
use std::path::Path;

use crate::error::{Error, Result};

/// Maximum allowed file size for memory mapping (1 GB).
const MAX_MMAP_SIZE: usize = 1024 * 1024 * 1024;

/// Owned memory-mapped string.
///
/// Holds the `Mmap` so the kernel mapping outlives any borrowed `&str` returned
/// by [`MmapStr::as_str`]. UTF-8 validity is established at construction.
#[derive(Debug)]
pub struct MmapStr {
    mmap: memmap2::Mmap,
    // Keep the file alive so its fd remains valid for the advisory lock.
    #[cfg(unix)]
    file: File,
}

impl MmapStr {
    /// Memory-map a file and validate its content as UTF-8.
    ///
    /// # Errors
    /// Returns an error if the file cannot be opened, exceeds `MAX_MMAP_SIZE`,
    /// the mapping fails, or the bytes are not valid UTF-8.
    pub fn open(path: &Path) -> Result<Self> {
        let file = File::open(path).map_err(Error::Io)?;
        let metadata = file.metadata().map_err(Error::Io)?;
        let size = usize::try_from(metadata.len()).map_err(|_| {
            Error::Generic(format!("File too large to address: {}", metadata.len()))
        })?;

        if size > MAX_MMAP_SIZE {
            return Err(Error::validation(
                format!("File size {size} exceeds maximum allowed size of {MAX_MMAP_SIZE} bytes"),
                "file_size",
                size.to_string(),
            ));
        }

        // Acquire an advisory read lock to defend against concurrent writers.
        // This is a best-effort guard; writers that also obey flock are blocked
        // while the mapping is live, preventing torn reads.
        #[cfg(unix)]
        {
            let fd = file.as_raw_fd();
            let ret = unsafe { libc::flock(fd, libc::LOCK_SH | libc::LOCK_NB) };
            if ret != 0 {
                return Err(Error::Generic(format!(
                    "Failed to acquire advisory read lock on {}: {}",
                    path.display(),
                    std::io::Error::last_os_error()
                )));
            }
        }

        // SAFETY: `memmap2::Mmap::map` is unsafe because the file may be
        // mutated concurrently. We document that constraint at module level
        // and validate UTF-8 once below; callers that share the file with
        // writers must not rely on this type.
        let mmap = unsafe { memmap2::Mmap::map(&file).map_err(Error::Io)? };

        if std::str::from_utf8(&mmap).is_err() {
            return Err(Error::Generic(format!(
                "Invalid UTF-8 in file: {}",
                path.display()
            )));
        }

        #[cfg(unix)]
        {
            Ok(Self { mmap, file })
        }
        #[cfg(not(unix))]
        {
            Ok(Self { mmap })
        }
    }

    /// Borrow the mapped content as a string slice.
    ///
    /// Zero-copy: returns a slice into the mmap, valid for the lifetime of
    /// `&self`.
    #[must_use]
    pub fn as_str(&self) -> &str {
        // SAFETY: `MmapStr::open` validated UTF-8 over the full mapping, and the
        // bytes stay immutable for the lifetime of `self` under our usage model —
        // an advisory shared `flock` is held for the mapping's lifetime (see
        // `open`). The residual TOCTOU — a concurrent writer that ignores
        // the advisory lock could mutate the file and invalidate this assumption —
        // is an accepted caveat for source-scan use: inputs are read-only and the
        // writers in this codebase honor the lock.
        unsafe { std::str::from_utf8_unchecked(&self.mmap) }
    }

    /// Length of the mapped content in bytes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.mmap.len()
    }

    /// Whether the mapping is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.mmap.is_empty()
    }

    /// Convert into an owned `String` (allocates).
    #[must_use]
    pub fn to_owned_string(&self) -> String {
        self.as_str().to_string()
    }
}

#[cfg(unix)]
impl Drop for MmapStr {
    fn drop(&mut self) {
        let fd = self.file.as_raw_fd();
        unsafe {
            // Unlock the advisory lock acquired in `open`.  Ignore errors
            // because there is little a caller can do at drop time, and the
            // fd will be closed shortly after anyway.
            let _ = libc::flock(fd, libc::LOCK_UN);
        }
    }
}

impl AsRef<str> for MmapStr {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

/// Memory-mapped file reader with threshold-based selection.
///
/// Files smaller than `mmap_threshold` use `std::fs::read_to_string`; files
/// larger than `mmap_max_size` also fall back (very large files have other
/// pathologies under `mmap`). Files in between are memory-mapped.
#[derive(Debug, Clone)]
pub struct MmapReader {
    mmap_threshold: usize,
    mmap_max_size: usize,
}

impl Default for MmapReader {
    fn default() -> Self {
        Self {
            mmap_threshold: 10 * 1024 * 1024, // 10 MB
            mmap_max_size: 500 * 1024 * 1024, // 500 MB
        }
    }
}

impl MmapReader {
    /// Create a reader with custom thresholds.
    #[must_use]
    pub fn new(mmap_threshold: usize, mmap_max_size: usize) -> Self {
        Self {
            mmap_threshold,
            mmap_max_size,
        }
    }

    /// Read file content, automatically selecting the best method.
    ///
    /// # Errors
    /// Returns an error if the file cannot be read or contains invalid UTF-8.
    pub fn read_file(&self, path: &Path) -> Result<String> {
        let metadata = std::fs::metadata(path).map_err(Error::Io)?;
        let size = usize::try_from(metadata.len()).unwrap_or(usize::MAX);

        if size >= self.mmap_threshold && size <= self.mmap_max_size {
            let mmap_str = MmapStr::open(path)?;
            Ok(mmap_str.to_owned_string())
        } else {
            std::fs::read_to_string(path).map_err(Error::Io)
        }
    }

    /// Read a file as a borrowed [`MmapStr`] (zero-copy).
    ///
    /// # Errors
    /// See [`MmapStr::open`].
    pub fn read_file_zero_copy(&self, path: &Path) -> Result<MmapStr> {
        MmapStr::open(path)
    }

    /// The mmap threshold in bytes.
    #[must_use]
    pub fn mmap_threshold(&self) -> usize {
        self.mmap_threshold
    }

    /// The mmap max size in bytes.
    #[must_use]
    pub fn mmap_max_size(&self) -> usize {
        self.mmap_max_size
    }
}

/// Statistics for mmap operations.
#[derive(Debug, Clone, Default)]
pub struct MmapStats {
    /// Number of files read using mmap.
    pub mmap_reads: u64,
    /// Number of files read using regular I/O.
    pub regular_reads: u64,
    /// Total bytes read via mmap.
    pub mmap_bytes: u64,
    /// Total bytes read via regular I/O.
    pub regular_bytes: u64,
}

impl MmapStats {
    /// Total reads (mmap + regular).
    #[must_use]
    pub fn total_reads(&self) -> u64 {
        self.mmap_reads + self.regular_reads
    }

    /// Total bytes read (mmap + regular).
    #[must_use]
    pub fn total_bytes(&self) -> u64 {
        self.mmap_bytes + self.regular_bytes
    }

    /// Percentage (0..=100) of reads served by mmap.
    #[must_use]
    pub fn mmap_percentage(&self) -> f64 {
        let total = self.total_reads();
        if total == 0 {
            0.0
        } else {
            #[allow(clippy::cast_precision_loss)]
            let pct = (self.mmap_reads as f64 / total as f64) * 100.0;
            pct
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_mmap_str_zero_copy() {
        let mut temp_file = NamedTempFile::new().unwrap();
        temp_file.write_all(b"Hello, World!").unwrap();

        let mmap_str = MmapStr::open(temp_file.path()).unwrap();
        assert_eq!(mmap_str.as_str(), "Hello, World!");
        assert_eq!(mmap_str.len(), 13);
        assert!(!mmap_str.is_empty());
    }

    #[test]
    fn test_mmap_str_empty_file() {
        let temp_file = NamedTempFile::new().unwrap();

        let mmap_str = MmapStr::open(temp_file.path()).unwrap();
        assert_eq!(mmap_str.as_str(), "");
        assert_eq!(mmap_str.len(), 0);
        assert!(mmap_str.is_empty());
    }

    #[test]
    fn test_mmap_str_as_ref() {
        let mut temp_file = NamedTempFile::new().unwrap();
        temp_file.write_all(b"test content").unwrap();

        let mmap_str = MmapStr::open(temp_file.path()).unwrap();
        let content: &str = mmap_str.as_ref();
        assert_eq!(content, "test content");
    }

    #[test]
    fn test_mmap_reader_regular_read() {
        let mut temp_file = NamedTempFile::new().unwrap();
        temp_file.write_all(b"Small content").unwrap();

        let reader = MmapReader::new(1024, 1024 * 1024); // 1 KB threshold
        let content = reader.read_file(temp_file.path()).unwrap();
        assert_eq!(content, "Small content");
    }

    #[test]
    fn test_mmap_reader_zero_copy() {
        let mut temp_file = NamedTempFile::new().unwrap();
        temp_file.write_all(b"Zero copy test").unwrap();

        let reader = MmapReader::default();
        let mmap_str = reader.read_file_zero_copy(temp_file.path()).unwrap();
        assert_eq!(mmap_str.as_str(), "Zero copy test");
    }

    #[test]
    fn test_mmap_stats() {
        let stats = MmapStats {
            mmap_reads: 10,
            regular_reads: 90,
            mmap_bytes: 100_000,
            regular_bytes: 50_000,
        };

        assert_eq!(stats.total_reads(), 100);
        assert_eq!(stats.total_bytes(), 150_000);
        assert!((stats.mmap_percentage() - 10.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_mmap_stats_empty() {
        let stats = MmapStats::default();
        assert!(stats.mmap_percentage().abs() < f64::EPSILON);
    }

    #[test]
    fn test_mmap_str_invalid_utf8() {
        let mut temp_file = NamedTempFile::new().unwrap();
        temp_file.write_all(&[0xFF, 0xFE, 0xFD]).unwrap();

        let result = MmapStr::open(temp_file.path());
        assert!(result.is_err(), "Invalid UTF-8 should be rejected");
    }
}
