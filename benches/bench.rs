#![feature(test)]
extern crate rls_span;
extern crate rls_vfs;
extern crate test;

use rls_span::{Column, Position, Row, Span};
use rls_vfs::Change;
use std::fs;
use std::io::prelude::*;
use std::path::{Path, PathBuf};

struct EmptyUserData;
type Vfs = rls_vfs::Vfs<EmptyUserData>;

fn add_file(vfs: &Vfs, path: &Path) {
    let mut buf = String::new();
    let mut file = fs::File::open(path).unwrap();
    file.read_to_string(&mut buf).unwrap();
    let change = Change::AddFile {
        file: path.to_owned(),
        text: buf,
    };
    vfs.on_changes(&[change]).unwrap();
}

fn make_change_(path: &Path, start_line: usize, interval: usize) -> Change {
    const LEN: usize = 10;
    let txt = unsafe { std::str::from_utf8_unchecked(&[b' '; 100]) };
    let start = Position::new(
        Row::new_zero_indexed(start_line as u32),
        Column::new_zero_indexed(0),
    );
    let end = Position::new(
        Row::new_zero_indexed((start_line + interval) as u32),
        Column::new_zero_indexed(0),
    );
    let buf = (0..LEN).map(|_| txt.to_owned() + "\n").collect::<String>();
    Change::ReplaceText {
        span: Span::from_positions(start, end, path),
        len: None,
        text: buf,
    }
}

fn make_replace(path: &Path, start_line: usize) -> Change {
    make_change_(path, start_line, 10)
}

fn make_insertion(path: &Path, start_line: usize) -> Change {
    make_change_(path, start_line, 1)
}

fn prepare() -> (Vfs, PathBuf) {
    let vfs = Vfs::new();
    let lib = Path::new("resources").join("path.rs").to_owned();
    add_file(&vfs, &lib);
    (vfs, lib)
}

#[bench]
fn replace_front(b: &mut test::Bencher) {
    let (vfs, lib) = prepare();
    b.iter(|| {
        for _ in 0..10 {
            let change = make_replace(&lib, 0);
            vfs.on_changes(&[change]).unwrap();
        }
    })
}

#[bench]
fn replace_mid(b: &mut test::Bencher) {
    let (vfs, lib) = prepare();
    b.iter(|| {
        for _ in 0..10 {
            let change = make_replace(&lib, 2000);
            vfs.on_changes(&[change]).unwrap();
        }
    })
}

#[bench]
fn replace_tale(b: &mut test::Bencher) {
    let (vfs, lib) = prepare();
    b.iter(|| {
        for _ in 0..10 {
            let change = make_replace(&lib, 4000);
            vfs.on_changes(&[change]).unwrap();
        }
    })
}

#[bench]
fn insert_front(b: &mut test::Bencher) {
    let (vfs, lib) = prepare();
    b.iter(|| {
        for _ in 0..10 {
            let change = make_insertion(&lib, 0);
            vfs.on_changes(&[change]).unwrap();
        }
    })
}

#[bench]
fn insert_mid(b: &mut test::Bencher) {
    let (vfs, lib) = prepare();
    b.iter(|| {
        for _ in 0..10 {
            let change = make_insertion(&lib, 2000);
            vfs.on_changes(&[change]).unwrap();
        }
    })
}

#[bench]
fn insert_tale(b: &mut test::Bencher) {
    let (vfs, lib) = prepare();
    b.iter(|| {
        for _ in 0..10 {
            let change = make_insertion(&lib, 4000);
            vfs.on_changes(&[change]).unwrap();
        }
    })
}
