//! Shingle sets that can live on disk.
//!
//! Clustering needs a document's full shingle set only for candidate pairs
//! that survive exact length and prefix bounds
//! ([`crate::near::shared_upper_bound`]). Everything else is decided from
//! the set's length and a sorted prefix, so the prefix stays in memory while
//! the rest of the set can sit in an anonymous temporary file.

use std::borrow::Cow;
use std::fs::File;
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Hash values below this bound -- an eighth of the `u64` range, so about
/// an eighth of every set -- always stay in memory.
pub(crate) const RESIDENT_BELOW: u64 = 1 << 61;
/// At least this many smallest values stay in memory: the bottom-k sketch
/// that candidate generation reads for every document.
pub(crate) const MIN_PREFIX: usize = crate::family::SKETCH_K;

/// Append-only home for the on-disk part of shingle sets.
///
/// Backed by an anonymous temporary file that the operating system removes
/// once the store is dropped or the process exits. Writes and reads are
/// positional, so any number of threads can use one store at once.
#[derive(Debug)]
pub struct ShingleStore {
    file: File,
    end: AtomicU64,
}

impl ShingleStore {
    /// A store in the system temporary directory (`TMPDIR` on Unix).
    pub fn temp() -> io::Result<Self> {
        Ok(Self {
            file: tempfile::tempfile()?,
            end: AtomicU64::new(0),
        })
    }

    /// Bytes appended so far.
    pub fn bytes_written(&self) -> u64 {
        self.end.load(Ordering::Relaxed)
    }

    fn append(&self, values: &[u64]) -> io::Result<u64> {
        let mut bytes = Vec::with_capacity(values.len() * 8);
        for value in values {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        let offset = self.end.fetch_add(bytes.len() as u64, Ordering::Relaxed);
        write_all_at(&self.file, &bytes, offset)?;
        Ok(offset)
    }

    fn read_into(&self, offset: u64, count: usize, out: &mut Vec<u64>) -> io::Result<()> {
        let mut bytes = vec![0u8; count * 8];
        read_exact_at(&self.file, &mut bytes, offset)?;
        out.extend(
            bytes
                .chunks_exact(8)
                .map(|b| u64::from_le_bytes(b.try_into().expect("8-byte chunk"))),
        );
        Ok(())
    }
}

/// One document's sorted, deduplicated shingle hashes.
///
/// The smallest values ([`Shingles::prefix`]) are always in memory. The
/// rest is either in memory too ([`Shingles::resident`]) or in a
/// [`ShingleStore`] ([`Shingles::stored`]) and read back on demand.
#[derive(Debug, Clone)]
pub struct Shingles(Repr);

#[derive(Debug, Clone)]
enum Repr {
    Resident {
        values: Vec<u64>,
        prefix_len: usize,
    },
    Stored {
        prefix: Vec<u64>,
        len: usize,
        store: Arc<ShingleStore>,
        rest_offset: u64,
    },
}

impl Shingles {
    /// Keep the whole set in memory.
    pub fn resident(values: Vec<u64>) -> Self {
        let prefix_len = prefix_len(&values);
        Self(Repr::Resident { values, prefix_len })
    }

    /// Keep the prefix in memory and append the rest to `store`. A set that
    /// is entirely prefix stays resident and writes nothing.
    pub fn stored(mut values: Vec<u64>, store: &Arc<ShingleStore>) -> io::Result<Self> {
        let prefix_len = prefix_len(&values);
        if prefix_len == values.len() {
            return Ok(Self::resident(values));
        }
        let rest_offset = store.append(&values[prefix_len..])?;
        let len = values.len();
        values.truncate(prefix_len);
        values.shrink_to_fit();
        Ok(Self(Repr::Stored {
            prefix: values,
            len,
            store: Arc::clone(store),
            rest_offset,
        }))
    }

    pub fn len(&self) -> usize {
        match &self.0 {
            Repr::Resident { values, .. } => values.len(),
            Repr::Stored { len, .. } => *len,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The smallest values: at least `min(len, SKETCH_K)` of them plus every
    /// value below an eighth of the hash range. Always in memory.
    pub fn prefix(&self) -> &[u64] {
        match &self.0 {
            Repr::Resident { values, prefix_len } => &values[..*prefix_len],
            Repr::Stored { prefix, .. } => prefix,
        }
    }

    /// The full sorted set, read back from the store when not resident.
    pub fn load(&self) -> io::Result<Cow<'_, [u64]>> {
        match &self.0 {
            Repr::Resident { values, .. } => Ok(Cow::Borrowed(values)),
            Repr::Stored {
                prefix,
                len,
                store,
                rest_offset,
            } => {
                let mut values = Vec::with_capacity(*len);
                values.extend_from_slice(prefix);
                store.read_into(*rest_offset, len - prefix.len(), &mut values)?;
                Ok(Cow::Owned(values))
            }
        }
    }
}

impl From<Vec<u64>> for Shingles {
    fn from(values: Vec<u64>) -> Self {
        Self::resident(values)
    }
}

fn prefix_len(values: &[u64]) -> usize {
    values
        .partition_point(|&h| h < RESIDENT_BELOW)
        .max(MIN_PREFIX)
        .min(values.len())
}

#[cfg(unix)]
fn write_all_at(file: &File, buf: &[u8], offset: u64) -> io::Result<()> {
    std::os::unix::fs::FileExt::write_all_at(file, buf, offset)
}

#[cfg(unix)]
fn read_exact_at(file: &File, buf: &mut [u8], offset: u64) -> io::Result<()> {
    std::os::unix::fs::FileExt::read_exact_at(file, buf, offset)
}

#[cfg(windows)]
fn write_all_at(file: &File, mut buf: &[u8], mut offset: u64) -> io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !buf.is_empty() {
        match file.seek_write(buf, offset) {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(n) => {
                buf = &buf[n..];
                offset += n as u64;
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

#[cfg(windows)]
fn read_exact_at(file: &File, mut buf: &mut [u8], mut offset: u64) -> io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !buf.is_empty() {
        match file.seek_read(buf, offset) {
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => {
                buf = &mut buf[n..];
                offset += n as u64;
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn sorted_values(n: usize, salt: u64) -> Vec<u64> {
        let mut v: Vec<u64> = (0..n as u64)
            .map(|i| (i ^ salt).wrapping_mul(0x9E37_79B9_7F4A_7C15))
            .collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    #[test]
    fn stored_set_reads_back_exactly() {
        let store = Arc::new(ShingleStore::temp().unwrap());
        let values = sorted_values(5_000, 1);
        let stored = Shingles::stored(values.clone(), &store).unwrap();
        assert_eq!(stored.len(), values.len());
        assert_eq!(&*stored.load().unwrap(), values.as_slice());
    }

    #[test]
    fn prefix_holds_the_smallest_values_in_memory() {
        let store = Arc::new(ShingleStore::temp().unwrap());
        let values = sorted_values(5_000, 2);
        for shingles in [
            Shingles::stored(values.clone(), &store).unwrap(),
            Shingles::resident(values.clone()),
        ] {
            let prefix = shingles.prefix();
            assert_eq!(prefix, &values[..prefix.len()]);
            assert!(prefix.len() >= MIN_PREFIX);
            // Every value in the resident range is in the prefix.
            let resident = values.iter().filter(|&&h| h < RESIDENT_BELOW).count();
            assert_eq!(prefix.len(), resident.max(MIN_PREFIX));
            // Roughly an eighth of uniformly spread hashes stay resident.
            assert!(prefix.len() < values.len() / 4, "{}", prefix.len());
        }
    }

    #[test]
    fn short_sets_are_kept_whole_in_memory() {
        let store = Arc::new(ShingleStore::temp().unwrap());
        let values = sorted_values(MIN_PREFIX - 10, 3);
        let shingles = Shingles::stored(values.clone(), &store).unwrap();
        assert_eq!(shingles.prefix(), values.as_slice());
        assert_eq!(&*shingles.load().unwrap(), values.as_slice());
        assert_eq!(store.bytes_written(), 0, "a complete prefix needs no disk");
        let empty = Shingles::stored(Vec::new(), &store).unwrap();
        assert!(empty.is_empty());
        assert!(empty.prefix().is_empty());
    }

    #[test]
    fn concurrent_writers_keep_every_set_intact() {
        let store = Arc::new(ShingleStore::temp().unwrap());
        let sets: Vec<Vec<u64>> = (0..64)
            .map(|i| sorted_values(500 + i * 37, i as u64))
            .collect();
        let stored: Vec<Shingles> = std::thread::scope(|s| {
            let handles: Vec<_> = sets
                .iter()
                .map(|set| {
                    let store = Arc::clone(&store);
                    s.spawn(move || Shingles::stored(set.clone(), &store).unwrap())
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        for (set, shingles) in sets.iter().zip(&stored) {
            assert_eq!(&*shingles.load().unwrap(), set.as_slice());
        }
    }
}
