use crate::cartridge::{ReadBytes, ReadBytesExt};
use crate::io_bail;
use std::collections::VecDeque;
use std::fs;
use std::io;
use std::io::{Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// An iterator that traverses the entries of a [filesystem](`Filesystem`)
/// in pre-order.
///
/// If cycles are present, then `FsTraversal` is an infinite iterator.
pub struct FsTraversal<'a> {
    depth: usize,
    dir_files: VecDeque<&'a Entry>,
    to_visit: VecDeque<(usize, &'a Entry)>,
}

impl<'a> FsTraversal<'a> {
    /// Returns an iterator that traverses filesystem entries
    /// recursively, starting at a given directory.
    ///
    /// The iterator returns `None` if the directory is empty. Otherwise,
    /// returns a pair containing the depth of the entry relative to the
    /// given directory and the entry itself.
    pub fn new(start: &'a Directory) -> Self {
        let mut dir_files = VecDeque::new();
        let mut to_visit = VecDeque::new();
        for entry in start.entries() {
            if let Entry::File(_) = entry {
                dir_files.push_back(entry);
            } else {
                to_visit.push_back((1, entry));
            }
        }

        Self {
            depth: 0,
            dir_files,
            to_visit,
        }
    }
}

impl<'a> Iterator for FsTraversal<'a> {
    type Item = (usize, &'a Entry);

    fn next(&mut self) -> Option<Self::Item> {
        // First, we visit the files in the current directory.
        if let Some(file) = self.dir_files.pop_front() {
            return Some((self.depth, file));
        }

        // Once we run out of files, we visit the next subdirectory.
        if let Some((depth, dir_entry @ Entry::Directory(dir))) = self.to_visit.pop_back() {
            for entry in dir.entries() {
                match entry {
                    Entry::File(_) => self.dir_files.push_front(entry),
                    Entry::Directory(_) => self.to_visit.push_back((depth + 1, entry)),
                }
            }
            self.depth = depth;
            Some((depth, dir_entry))
        } else {
            // We traversed all the subdirectories.
            None
        }
    }
}

/// A directory stored within a NitroROM filesystem, that contains
/// zero or more [entries](`Entry`).
#[derive(Debug)]
pub struct Directory {
    name: String,
    entries: Vec<Entry>,
}

impl Directory {
    /// Attempts to read the contents of a directory with the given name.
    ///
    /// The file cursor must point to the FNT main table entry of the directory.
    /// On success, the file cursor is positioned at the end of the FNT sub-table
    /// of the directory.
    pub fn read(fs: &mut Filesystem, name: String) -> io::Result<Self> {
        let sub_table_offset = fs.fnt_offset + fs.inner.read_u32()?;
        let first_file_id = fs.inner.read_u16()?;
        // We ignore the parent dir ID/total # of dirs fields.

        fs.inner.seek(SeekFrom::Start(sub_table_offset as u64))?;
        Ok(Directory {
            name,
            entries: Entry::read_sub_table(fs, first_file_id)?,
        })
    }

    /// Returns the name of the directory.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns an iterator over the directory entries.
    pub fn entries(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter()
    }

    /// Traverses the files within the directory recursively in pre-order.
    pub fn traverse(&self) -> FsTraversal {
        FsTraversal::new(self)
    }

    /// Searches for a filesystem entry at the given path relative
    /// to this directory.
    pub fn search<P: AsRef<Path>>(&self, path: P) -> Option<&Entry> {
        let mut curr_depth = 0;
        let mut stack = PathBuf::new();
        for (depth, entry) in self.traverse() {
            match entry {
                Entry::Directory(dir) => {
                    // Pop directory names from the stack until we reach
                    // a common parent level. For example, if a sibling
                    // directory is traversed, then `depth == curr_depth`,
                    // so we pop once. If a sub-directory is traversed, then
                    // `curr_depth < depth` so do nothing.
                    while curr_depth >= depth {
                        assert!(stack.pop());
                        curr_depth -= 1;
                    }

                    // We're at the parent level, push new trailing directory.
                    stack.push(dir.name());
                    curr_depth = depth;

                    // Check if the directory matches the search path.
                    if stack == path.as_ref() {
                        return Some(entry);
                    }
                }
                Entry::File(file) => {
                    // Check if the file matches the search path.
                    stack.push(file.name());
                    if stack == path.as_ref() {
                        return Some(entry);
                    }
                    stack.pop();
                }
            }
        }
        None
    }
}

/// A file stored within a NitroROM filesystem.
#[derive(Debug)]
pub struct File {
    name: String,
    offset: u32,
    len: u32,
}

impl File {
    /// Attempts to read the metadata of a file with the given name.
    ///
    /// The file cursor must point to the start of the FAT entry of the file.
    /// On success, the file cursor is positioned at the end of the FAT entry.
    pub fn read(fs: &mut Filesystem, name: String) -> io::Result<Self> {
        let offset = fs.inner.read_u32()?; // with respect to image base
        Ok(Self {
            name,
            offset: fs.image_offset + offset,
            len: fs.inner.read_u32()? - offset, // todo: + 1?
        })
    }

    /// Returns the name of the file.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the offset within the cartridge at which the file
    /// contents start.
    pub fn offset(&self) -> u32 {
        self.offset
    }

    /// Returns the length of the file contents in bytes.
    pub fn len(&self) -> u32 {
        self.len
    }
}

/// A filesystem entry.
#[derive(Debug)]
pub enum Entry {
    Directory(Directory),
    File(File),
}

impl Entry {
    /// Attempts to read the entries of a FNT sub-table.
    ///
    /// The file cursor must point to the start of the first sub-table entry.
    /// The `file_id` is the ID of the first file within the sub-table, if any.
    ///
    /// On success, the file cursor is positioned at the end of the sub-table.
    pub fn read_sub_table(fs: &mut Filesystem, mut file_id: u16) -> io::Result<Vec<Self>> {
        let mut entries = Vec::new();
        loop {
            // Each entry contains a 1-byte header that specifies whether the entry
            // is a file or a sub-directory and the file name length. The file name
            // follows the header. If the entry is a sub-directory, its index into
            // the FNT main table follows the directory name. File entries do not
            // contain any ID field, since they are assigned in increasing order
            // within the directory, starting at `file_id`.
            let header = fs.inner.read_u8()?;
            if header == 0 {
                break; // reached the end of the sub-table.
            }
            if header == 0x80 {
                continue; // reserved.
            }
            let name = fs.inner.read_string((header & 0x7F) as usize)?;

            let mut entry_end = fs.inner.stream_position()?;
            entries.push(if (header & 0x80) == 0 {
                // Read the file entry. Its ID is an offset into the FAT.
                fs.inner.seek(SeekFrom::Start(fs.fat_offset(file_id)))?;
                file_id += 1;

                Self::File(File::read(fs, name)?)
            } else {
                // Read the sub-directory recursively.
                let subdir_id = fs.inner.read_u16()? & 0xFFF;
                entry_end += 2;

                fs.inner.seek(SeekFrom::Start(fs.fnt_offset(subdir_id)))?;
                Self::Directory(Directory::read(fs, name)?)
            });

            // Parsing the FAT or the sub-table leaves the position of the file cursor
            // unspecified. Restore it so that the next entry is read correctly.
            fs.inner.seek(SeekFrom::Start(entry_end))?;
        }
        Ok(entries)
    }

    /// Returns the wrapped object if this entry is a directory.
    pub fn directory(&self) -> Option<&Directory> {
        if let Entry::Directory(dir) = self {
            Some(dir)
        } else {
            None
        }
    }

    /// Returns the wrapped object if this entry is a file.
    pub fn file(&self) -> Option<&File> {
        if let Entry::File(file) = self {
            Some(file)
        } else {
            None
        }
    }
}

/// A Nitro Archive (NARC) chunk description.
#[derive(Debug)]
struct NitroArcChunk {
    /// The offset within the cartridge at which the chunk starts.
    offset: u32,
    /// The chunk length in bytes (including the header).
    len: u32,
}

impl NitroArcChunk {
    /// Reads a NARC chunk with the expected name.
    ///
    /// On success, the file cursor is positioned at the end of the chunk.
    pub fn read(file: &mut fs::File, name: &str) -> io::Result<Self> {
        let offset = file.stream_position()? as u32;
        let actual_name = file.read_string(4)?;
        if actual_name != name {
            io_bail!(
                "incorrect NARC chunk name '{}', expected '{}'",
                actual_name,
                name
            );
        }
        let len = file.read_u32()?;
        file.seek(SeekFrom::Start((offset + len) as u64))?; // skip contents.
        Ok(NitroArcChunk {
            offset,
            len: file.read_u32()?,
        })
    }
}

/// The contents of a NitroROM filesystem.
#[derive(Debug)]
pub struct Filesystem<'a> {
    inner: &'a mut fs::File,
    /// The offset within the cartridge at which the File Name Table (FNT) starts.
    fnt_offset: u32,
    /// The offset within the cartridge at which the File Allocation Table (FAT) starts.
    fat_offset: u32,
    /// The offset within the cartridge at which the image area starts.
    ///
    /// The image area of the main NitroROM filesystem always starts at zero.
    /// However, the console checks that all files are stored outside of
    /// the Secure Area (at `0x8000` and up). This implementation is lenient
    /// and allows such offsets.
    image_offset: u32,
}

impl<'a> Filesystem<'a> {
    /// Attempts to read the main NitroROM filesystem of a cartridge.
    ///
    /// The current file cursor is not affected by this function.
    pub fn from_rom(file: &'a mut fs::File) -> io::Result<Self> {
        let fnt_offset = file.read_u32_at(0x40)?;
        let fat_offset = file.read_u32_at(0x48)?;
        Ok(Self {
            inner: file,
            fnt_offset,
            fat_offset,
            // FAT offsets are relative to the ROM start.
            image_offset: 0,
        })
    }

    /// Attempts to read a Nitro Archive (NARC) virtual filesystem starting from
    /// the current file cursor position.
    ///
    /// On success, the file cursor is positioned at the end of the archive.
    /// If this function returns an error, the cursor position is unspecified.
    pub fn from_archive(file: &'a mut fs::File) -> io::Result<Self> {
        // A NARC is composed of a header and 3 chunks: the FAT, the FNT
        // and the image containing the file contents.
        let file_sig = file.read_string(4)?;
        if file_sig != "NARC" {
            io_bail!("incorrect file signature '{}', expected 'NARC'", file_sig);
        }
        file.skip(2)?; // byte order
        let version = file.read_u16()?;
        if version != 0x10 {
            io_bail!("unknown NARC file version {}", version);
        }
        file.skip(6)?; // skip file and header size
        let chunk_count = file.read_u16()?;
        if chunk_count != 3 {
            io_bail!("NARC file has {} chunk, expected 3", chunk_count);
        }

        let fat = NitroArcChunk::read(file, "BTAF")?;
        let fnt = NitroArcChunk::read(file, "BTNF")?;
        let image = NitroArcChunk::read(file, "GMIF")?;

        Ok(Self {
            inner: file,
            fnt_offset: fnt.offset,
            fat_offset: fat.offset,
            image_offset: image.offset,
        })
    }

    /// Returns the offset within the cartridge at which the FAT entry for
    /// the file with the given ID starts.
    fn fat_offset(&self, file_id: u16) -> u64 {
        self.fat_offset as u64 + (file_id as u64) * 8
    }

    /// Returns the offset within the cartridge at which the FNT main table
    /// entry for the directory with the given ID starts.
    fn fnt_offset(&self, dir_id: u16) -> u64 {
        self.fnt_offset as u64 + (dir_id as u64) * 8
    }

    /// Attempts to read the contents of the root directory.
    ///
    /// The file cursor position is unspecified upon return.
    pub fn root_dir(&mut self) -> io::Result<Directory> {
        // Each file in the filesystem has a unique ID ranging from 0 to
        // `self.file_count`. The FNT consists of a set of sub-tables that
        // contain the names of all entries (files and sub-directories) within
        // each directory; and the main table, that contains the starting file
        // IDs of each directory and an offset into its corresponding sub-table.
        // That is, file IDs are assigned sequentially within each directory.

        // The first main table entry corresponds to the root directory, and
        // unlike all the other entries, its third `u16` value contains the
        // total number of directories, not the ID of the parent directory.
        self.inner.seek(SeekFrom::Start(self.fnt_offset as u64))?;
        Directory::read(self, "root".to_string())
    }
}
