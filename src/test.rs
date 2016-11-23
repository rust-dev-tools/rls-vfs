use super::{VfsInternal, Change, FileLoader, File, Error};
use std::path::{Path, PathBuf};

struct MockFileLoader;

impl FileLoader for MockFileLoader {
    fn read<U>(file_name: &Path) -> Result<File<U>, Error> {
        let text = format!("{}\nHello\nWorld\nHello, World!\n", file_name.display());
        Ok(File::from_text(text))
    }

    fn write<U>(file_name: &Path, file: &File<U>) -> Result<(), Error> {
        if file_name.display().to_string() == "foo" {
            assert_eq!(file.changed, true);
            assert_eq!(file.text, "foo\nHfooo\nWorld\nHello, World!\n");
        }

        Ok(())
    }
}

fn make_change() -> Change {
    Change {
        file_name: Path::new("foo").into(),
        span: ((1, 1), (1, 4)).into(),
        text: "foo".to_owned(),
    }
}

fn make_change_2() -> Change {
    Change {
        file_name: Path::new("foo").into(),
        span: ((2, 4), (3, 2)).into(),
        text: "aye carumba".to_owned(),
    }
}

#[test]
fn test_has_changes() {
    let vfs = VfsInternal::<MockFileLoader, ()>::new();

    assert!(!vfs.has_changes());
    vfs.load_file(&Path::new("foo")).unwrap();
    assert!(!vfs.has_changes());
    vfs.on_changes(&[make_change()]).unwrap();
    assert!(vfs.has_changes());
    vfs.file_saved(&Path::new("bar")).unwrap();
    assert!(vfs.has_changes());
    vfs.file_saved(&Path::new("foo")).unwrap();
    assert!(!vfs.has_changes());
}

#[test]
fn test_cached_files() {
    let vfs = VfsInternal::<MockFileLoader, ()>::new();
    assert!(vfs.get_cached_files().is_empty());
    vfs.load_file(&Path::new("foo")).unwrap();
    vfs.load_file(&Path::new("bar")).unwrap();
    let files = vfs.get_cached_files();
    assert_eq!(files.len(), 2);
    assert_eq!(files[Path::new("foo")], "foo\nHello\nWorld\nHello, World!\n");
    assert_eq!(files[Path::new("bar")], "bar\nHello\nWorld\nHello, World!\n");
}

#[test]
fn test_flush_file() {
    let vfs = VfsInternal::<MockFileLoader, ()>::new();
    // Flushing an uncached-file should succeed.
    vfs.flush_file(&Path::new("foo")).unwrap();
    vfs.load_file(&Path::new("foo")).unwrap();
    vfs.flush_file(&Path::new("foo")).unwrap();
    assert!(vfs.get_cached_files().is_empty());
}

#[test]
fn test_changes() {
    let vfs = VfsInternal::<MockFileLoader, ()>::new();

    vfs.on_changes(&[make_change()]).unwrap();
    let files = vfs.get_cached_files();
    assert_eq!(files.len(), 1);
    assert_eq!(files[&PathBuf::from("foo")], "foo\nHfooo\nWorld\nHello, World!\n");
    assert_eq!(vfs.load_file(&Path::new("foo")), Ok("foo\nHfooo\nWorld\nHello, World!\n".to_owned()));
    assert_eq!(vfs.load_file(&Path::new("bar")), Ok("bar\nHello\nWorld\nHello, World!\n".to_owned()));

    vfs.on_changes(&[make_change_2()]).unwrap();
    let files = vfs.get_cached_files();
    assert_eq!(files.len(), 2);
    assert_eq!(files[&PathBuf::from("foo")], "foo\nHfooo\nWorlaye carumballo, World!\n");
    assert_eq!(vfs.load_file(&Path::new("foo")), Ok("foo\nHfooo\nWorlaye carumballo, World!\n".to_owned()));
}

#[test]
fn test_user_data() {
    let vfs = VfsInternal::<MockFileLoader, i32>::new();

    // New files have no user data.
    vfs.load_file(&Path::new("foo")).unwrap();
    vfs.with_user_data(&Path::new("foo"), |u| {
        assert_eq!(u, Err(Error::NoUserDataForFile));
    });

    // Set and read data.
    vfs.set_user_data(&Path::new("foo"), Some(42)).unwrap();
    vfs.with_user_data(&Path::new("foo"), |u| {
        assert_eq!(u, Ok(&42));
    });
    assert_eq!(vfs.set_user_data(&Path::new("bar"), Some(42)), Err(Error::FileNotCached));

    // compute and read data.
    vfs.compute_user_data(&Path::new("foo"), |s| {
        assert_eq!(s, "foo\nHello\nWorld\nHello, World!\n");
        Ok(43)
    }).unwrap();
    vfs.with_user_data(&Path::new("foo"), |u| {
        assert_eq!(u, Ok(&43));
    });
    assert_eq!(vfs.compute_user_data(&Path::new("foo"), |_| {
        Err(Error::BadLocation)
    }), Err(Error::BadLocation));
    vfs.with_user_data(&Path::new("foo"), |u| {
        assert_eq!(u, Ok(&43));
    });

    // Clear and read data.
    vfs.set_user_data(&Path::new("foo"), None).unwrap();
    vfs.with_user_data(&Path::new("foo"), |u| {
        assert_eq!(u, Err(Error::NoUserDataForFile));
    });

    // Compute (clear) and read data.
    vfs.set_user_data(&Path::new("foo"), Some(42)).unwrap();
    vfs.compute_user_data(&Path::new("foo"), |_| {
        Err(Error::NoUserDataForFile)
    }).unwrap();
    vfs.with_user_data(&Path::new("foo"), |u| {
        assert_eq!(u, Err(Error::NoUserDataForFile));
    });

    // Flushing a file should clear user data.
    vfs.set_user_data(&Path::new("foo"), Some(42)).unwrap();
    vfs.flush_file(&Path::new("foo")).unwrap();
    vfs.load_file(&Path::new("foo")).unwrap();
    vfs.with_user_data(&Path::new("foo"), |u| {
        assert_eq!(u, Err(Error::NoUserDataForFile));
    });

    // Recording a change should clear user data.
    vfs.set_user_data(&Path::new("foo"), Some(42)).unwrap();
    vfs.on_changes(&[make_change()]).unwrap();
    vfs.with_user_data(&Path::new("foo"), |u| {
        assert_eq!(u, Err(Error::NoUserDataForFile));
    });
}

#[test]
fn test_write() {
    let vfs = VfsInternal::<MockFileLoader, ()>::new();

    vfs.on_changes(&[make_change()]).unwrap();
    vfs.write_file(&Path::new("foo")).unwrap();
    let files = vfs.get_cached_files();
    assert!(files.len() == 1);
    let files = vfs.get_changes();
    assert!(files.is_empty());
}

// TODO test with wide chars
