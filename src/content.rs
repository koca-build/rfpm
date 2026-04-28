//! Unified content source for file and script data.
//!
//! [`Content`] wraps any `Read + Seek` source so callers can pass
//! [`File`], [`Vec<u8>`], [`String`], [`Cursor`], or any other seekable
//! reader without worrying about the underlying type.
//!
//! ```
//! use std::fs::File;
//! use rfpm::Content;
//!
//! // From a file on disk
//! let c: Content = File::open("Cargo.toml").unwrap().into();
//!
//! // From bytes
//! let c: Content = vec![1, 2, 3].into();
//!
//! // From a string
//! let c: Content = "#!/bin/sh\necho hello".to_string().into();
//! ```

use std::fs::File;
use std::io::{self, Cursor, Read, Seek, SeekFrom};

/// Trait alias for `Read + Seek`.
pub trait ReadSeek: Read + Seek {}
impl<T: Read + Seek> ReadSeek for T {}

/// A content source that can be read and seeked.
///
/// Wraps any `Read + Seek` type. Content can be read multiple times
/// by seeking back to the start, which is needed when writing the same
/// package in multiple formats.
///
/// Common types convert automatically via [`From`]:
/// - [`File`] — read from disk
/// - [`String`] / [`Vec<u8>`] — in-memory content (wrapped in [`Cursor`])
/// - [`Cursor<T>`] — any cursor over `AsRef<[u8]>` data
///
/// For other `Read + Seek` types, use [`Content::new`].
pub struct Content(Box<dyn ReadSeek>);

impl Content {
    /// Wrap any `Read + Seek` source as [`Content`].
    ///
    /// Use this for types that don't have a [`From`] impl.
    pub fn new(r: impl Read + Seek + 'static) -> Self {
        Content(Box::new(r))
    }

    /// Read all bytes from this content source, then seek back to the start.
    pub(crate) fn read_all(&mut self) -> io::Result<Vec<u8>> {
        self.0.seek(SeekFrom::Start(0))?;
        let mut buf = Vec::new();
        self.0.read_to_end(&mut buf)?;
        self.0.seek(SeekFrom::Start(0))?;
        Ok(buf)
    }

    /// Read content as a UTF-8 string, then seek back to the start.
    pub(crate) fn read_string(&mut self) -> io::Result<String> {
        let bytes = self.read_all()?;
        String::from_utf8(bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }
}

impl From<File> for Content {
    fn from(f: File) -> Self {
        Content(Box::new(f))
    }
}

impl<T: AsRef<[u8]> + 'static> From<Cursor<T>> for Content {
    fn from(c: Cursor<T>) -> Self {
        Content(Box::new(c))
    }
}

impl From<String> for Content {
    fn from(s: String) -> Self {
        Content(Box::new(Cursor::new(s.into_bytes())))
    }
}

impl From<Vec<u8>> for Content {
    fn from(v: Vec<u8>) -> Self {
        Content(Box::new(Cursor::new(v)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_from_vec() {
        let mut c: Content = vec![1, 2, 3].into();
        assert_eq!(c.read_all().unwrap(), vec![1, 2, 3]);
        // Can read again after automatic seek-back
        assert_eq!(c.read_all().unwrap(), vec![1, 2, 3]);
    }

    #[test]
    fn content_from_string() {
        let mut c: Content = "hello".to_string().into();
        assert_eq!(c.read_string().unwrap(), "hello");
        assert_eq!(c.read_string().unwrap(), "hello");
    }

    #[test]
    fn content_from_cursor() {
        let cursor = Cursor::new(vec![4, 5, 6]);
        let mut c: Content = cursor.into();
        assert_eq!(c.read_all().unwrap(), vec![4, 5, 6]);
        assert_eq!(c.read_all().unwrap(), vec![4, 5, 6]);
    }

    #[test]
    fn content_from_file() {
        let f = File::open("Cargo.toml").unwrap();
        let mut c: Content = f.into();
        let first = c.read_all().unwrap();
        let second = c.read_all().unwrap();
        assert_eq!(first, second);
        assert!(!first.is_empty());
    }

    #[test]
    fn content_new_escape_hatch() {
        let cursor = Cursor::new(b"test data".to_vec());
        let mut c = Content::new(cursor);
        assert_eq!(c.read_all().unwrap(), b"test data");
    }

    #[test]
    fn content_read_string_invalid_utf8() {
        let mut c: Content = vec![0xFF, 0xFE].into();
        assert!(c.read_string().is_err());
    }
}
