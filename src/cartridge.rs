use crate::Filesystem;
use paste::paste;
use std::fs::File;
use std::io;
use std::io::{Read, Seek, SeekFrom};
#[cfg(unix)]
use std::os::unix::fs::FileExt;
use std::path::Path;

// See https://github.com/intellij-rust/intellij-rust/issues/6908 to let
// the IntelliJ Rust plugin expand this macro.
macro_rules! impl_read_bytes {
    ($($ty:ident)+) => ($(
        paste! {
            /// Reads a `$ty` value encoded in little-endian order.
            fn [<read_ $ty>](&mut self) -> io::Result<$ty> {
                let mut buf = [0u8; std::mem::size_of::<$ty>()];
                self.read_exact(&mut buf)?;
                Ok($ty::from_le_bytes(buf))
            }
        }
    )+)
}

/// Binary-value extensions to [`Read`].
pub trait ReadBytes: Read {
    impl_read_bytes! {u8 i8 u16 i16 u32 i32 u64 i64 f32 f64}

    /// Reads a UTF-8 encoded string.
    fn read_string(&mut self, len: usize) -> io::Result<String> {
        let mut buf = vec![0u8; len];
        self.read_exact(&mut buf)?;
        String::from_utf8(buf).map_err(|err| io::Error::new(io::ErrorKind::Other, err))
    }

    /// Discards `offset` bytes from this source.
    fn skip(&mut self, offset: u64) -> io::Result<()>;
}

impl<T> ReadBytes for T
where
    T: Read,
{
    default fn skip(&mut self, offset: u64) -> io::Result<()> {
        io::copy(&mut self.take(offset), &mut io::sink())?;
        Ok(())
    }
}

impl<T> ReadBytes for T
where
    T: Read + Seek,
{
    fn skip(&mut self, offset: u64) -> io::Result<()> {
        self.seek(SeekFrom::Current(offset as i64))?;
        Ok(())
    }
}

macro_rules! impl_read_bytes_ext {
    ($($ty:ident)+) => ($(
        paste! {
            /// Reads a `$ty` value encoded in little-endian order starting
            /// from a given offset.
            fn[<read_ $ty _at>](&self, offset: u64) -> io::Result<$ty> {
                let mut buf = [0u8; std::mem::size_of::<$ty>()];
                self.read_exact_at(&mut buf, offset)?;
                Ok($ty::from_le_bytes(buf))
            }
        }
    )+)
}

/// Binary-value extensions to [`FileExt`].
///
/// The offset taken by the trait functions is relative to the start of the file
/// and thus independent from the current cursor. The current file cursor is
/// not affected by the trait functions.
pub trait ReadBytesExt: FileExt {
    impl_read_bytes_ext! {u8 i8 u16 i16 u32 i32 u64 i64 f32 f64}

    /// Reads a `len`-byte long UTF-8 encoded string starting from a given offset.
    fn read_string_at(&self, len: usize, offset: u64) -> io::Result<String> {
        let mut buf = vec![0u8; len];
        self.read_exact_at(&mut buf, offset)?;
        String::from_utf8(buf).map_err(|err| io::Error::new(io::ErrorKind::Other, err))
    }
}

impl<T> ReadBytesExt for T where T: FileExt {}

/// The contents of a Nintendo DS cartridge ROM.
pub struct Cartridge {
    inner: File,
}

impl Cartridge {
    /// Attempts to read a cartridge file.
    pub fn open<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        Ok(Self {
            inner: File::open(path)?,
        })
    }

    /// Attempts to read the main NitroROM filesystem of the cartridge.
    pub fn file_system(&mut self) -> io::Result<Filesystem> {
        Filesystem::from_rom(&mut self.inner)
    }
}

/// Returns early with an [`io::Error`].
///
/// Inspired by the `bail!` macro from the [anyhow](https://docs.rs/anyhow/latest/src/anyhow/macros.rs.html#56-66)
/// library.
#[macro_export]
macro_rules! io_bail {
    ($msg:literal $(,)?) => {
        return Err(io::Error::new(io::ErrorKind::Other, $msg))
    };
    ($err:expr $(,)?) => {
        return Err(io::Error::new(io::ErrorKind::Other, $err))
    };
    ($fmt:expr, $($arg:tt)*) => {
        return Err(io::Error::new(io::ErrorKind::Other, format!($fmt, $($arg)*)))
    };
}
