#![feature(specialization)]
#![feature(assert_matches)]

use crate::cartridge::Cartridge;
use crate::nitro::{Entry, Filesystem};
use std::assert_matches::assert_matches;
use std::io;

mod cartridge;
mod nitro;

fn main() -> io::Result<()> {
    let mut cartridge = Cartridge::open("cartridges/heartgold.nds")?;

    let mut fs = cartridge.file_system()?;
    let root_dir = fs.root_dir()?;

    for (depth, entry) in root_dir.traverse() {
        let prefix = " ".repeat(depth * 2);
        println!("{}- {:?}", prefix, entry)
    }

    assert_matches!(
        root_dir.search("data/weather_sys.narc"),
        Some(Entry::File(_))
    );
    assert_matches!(root_dir.search("a/0/0/0"), Some(Entry::File(_)));

    Ok(())
}
