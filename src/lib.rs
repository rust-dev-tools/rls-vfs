#![feature(proc_macro)]
#![feature(question_mark)]

extern crate rls_analysis;
#[macro_use]
extern crate serde_derive;

use rls_analysis::Span;

use std::collections::HashMap;
use std::fs;
use std::marker::PhantomData;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

macro_rules! try_opt_loc {
    ($e:expr) => {
        match $e {
            Some(e) => e,
            None => return Err(Error::BadLocation),
        }
    }
}

pub struct Vfs(VfsInternal<RealFileLoader>);

#[derive(Debug, Deserialize, Serialize)]
pub struct Change {
    pub span: Span,
    pub text: String,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum Error {
    /// The given file has become out of sync with the filesystem.
    OutOfSync(PathBuf),
    /// IO error reading or writing the given path, 2nd arg is a message.
    Io(Option<PathBuf>, Option<String>),
    /// There are changes to the given file which have not been written to disk.
    UncommittedChanges(PathBuf),
    /// Client specified a location that is not within a file. I.e., a row or
    /// column not in the file.
    BadLocation,
    /// The requested file was not cached in the VFS.
    FileNotCached,
}

impl Vfs {
    /// Creates a new, empty VFS.
    pub fn new() -> Vfs {
        Vfs(VfsInternal::<RealFileLoader>::new())
    }

    /// Indicate that the current file as known to the VFS has been written to
    /// disk.
    pub fn file_saved(&self, path: &Path) -> Result<(), Error> {
        self.0.file_saved(path)
    }

    /// Removes a file from the VFS. Does not check if the file is synced with
    /// the disk. Does not check if the file exists.
    pub fn flush_file(&self, path: &Path) -> Result<(), Error> {
        self.0.flush_file(path)
    }

    pub fn file_is_synced(&self, path: &Path) -> Result<bool, Error> {
        self.0.file_is_synced(path)
    }

    /// Record a set of changes to the VFS.
    pub fn on_changes(&self, changes: &[Change]) -> Result<(), Error> {
        self.0.on_changes(changes)
    }

    /// Return all files in the VFS.
    pub fn get_cached_files(&self) -> HashMap<PathBuf, String> {
        self.0.get_cached_files()
    }

    pub fn get_changes(&self) -> HashMap<PathBuf, String> {
        self.0.get_changes()
    }

    /// Returns true if the VFS contains any changed files.
    pub fn has_changes(&self) -> bool {
        self.0.has_changes()
    }

    pub fn set_file(&self, path: &Path, text: &str) {
        self.0.set_file(path, text)
    }

    pub fn load_file(&self, path: &Path) -> Result<String, Error> {
        self.0.load_file(path)
    }

    pub fn load_line(&self, path: &Path, line: usize) -> Result<String, Error> {
        self.0.load_line(path, line)
    }

    pub fn write_file(&self, path: &Path) -> Result<(), Error> {
        self.0.write_file(path)
    }
}

struct VfsInternal<T> {
    files: Mutex<HashMap<PathBuf, File>>,
    loader: PhantomData<T>,
}

struct File {
    // FIXME(https://github.com/jonathandturner/rustls/issues/21) should use a rope.
    text: String,
    line_indices: Vec<u32>,
    changed: bool,
}

impl<T: FileLoader> VfsInternal<T> {
    fn new() -> VfsInternal<T> {
        VfsInternal {
            files: Mutex::new(HashMap::new()),
            loader: PhantomData,
        }
    }

    fn file_saved(&self, path: &Path) -> Result<(), Error> {
        let mut files = self.files.lock().unwrap();
        if let Some(ref mut f) = files.get_mut(path) {
            f.changed = false;
        }
        Ok(())
    }

    fn flush_file(&self, path: &Path) -> Result<(), Error> {
        let mut files = self.files.lock().unwrap();
        files.remove(path);
        Ok(())
    }

    fn file_is_synced(&self, path: &Path) -> Result<bool, Error> {
        let files = self.files.lock().unwrap();
        match files.get(path) {
            Some(f) => Ok(!f.changed),
            None => Err(Error::FileNotCached),
        }
    }

    fn on_changes(&self, changes: &[Change]) -> Result<(), Error> {
        for (file_name, changes) in coalesce_changes(changes) {
            let path = Path::new(file_name);
            {
                let mut files = self.files.lock().unwrap();
                if let Some(file) = files.get_mut(Path::new(path)) {
                    file.make_change(&changes)?;
                    continue;
                }
            }

             let mut file = T::read(Path::new(path))?;
            file.make_change(&changes)?;

            let mut files = self.files.lock().unwrap();
            files.insert(path.to_path_buf(), file);
        }

        Ok(())
    }

    fn set_file(&self, path: &Path, text: &str) {
        let file = File {
            text: text.to_owned(),
            line_indices: File::make_line_indices(text),
            changed: true,
        };

        let mut files = self.files.lock().unwrap();
        files.insert(path.to_owned(), file);
    }

    fn get_cached_files(&self) -> HashMap<PathBuf, String> {
        let files = self.files.lock().unwrap();
        files.iter().map(|(p, f)| (p.clone(), f.text.clone())).collect()
    }

    fn get_changes(&self) -> HashMap<PathBuf, String> {
        let files = self.files.lock().unwrap();
        files.iter().filter_map(|(p, f)| if f.changed { Some((p.clone(), f.text.clone())) } else { None }).collect()
    }

    fn has_changes(&self) -> bool {
        let files = self.files.lock().unwrap();
        files.values().any(|f| f.changed)
    }

    fn load_line(&self, path: &Path, line: usize) -> Result<String, Error> {
        let mut files = self.files.lock().unwrap();
        Self::ensure_file(&mut files, path)?;

        files[path].load_line(line).map(|s| s.to_owned())
    }

    fn load_file(&self, path: &Path) -> Result<String, Error> {
        let mut files = self.files.lock().unwrap();
        Self::ensure_file(&mut files, path)?;

        Ok(files[path].text.clone())
    }

    fn ensure_file(files: &mut HashMap<PathBuf, File>, path: &Path) -> Result<(), Error>{
        if !files.contains_key(path) {
            let file = T::read(path)?;
            files.insert(path.to_path_buf(), file);
        }
        Ok(())
    }

    // TODO
    fn write_file(&self, _path: &Path) -> Result<(), Error> {
        unimplemented!()
    }
}

fn coalesce_changes<'a>(changes: &'a [Change]) -> HashMap<&'a str, Vec<&'a Change>> {
    // Note that for any given file, we preserve the order of the changes.
    let mut result = HashMap::new();
    for c in changes {
        result.entry(&*c.span.file_name).or_insert(vec![]).push(c);
    }
    result
}

impl File {
    fn make_line_indices(text: &str) -> Vec<u32> {
        let mut result = vec![0];
        for (i, b) in text.bytes().enumerate() {
            if b == 0xA {
                result.push((i + 1) as u32);
            }
        }
        result
    }

    // TODO errors for unwraps
    fn make_change(&mut self, changes: &[&Change]) -> Result<(), Error> {
        for c in changes {
            let range = {
                let first_line = self.load_line(c.span.line_start).unwrap();
                let last_line = self.load_line(c.span.line_end).unwrap();

                let byte_start = self.line_indices[c.span.line_start] +
                                 byte_in_str(first_line, c.span.column_start).unwrap() as u32;
                let byte_end = self.line_indices[c.span.line_end] +
                               byte_in_str(last_line, c.span.column_end).unwrap() as u32;
                (byte_start, byte_end)
            };
            let mut new_text = self.text[..range.0 as usize].to_owned();
            new_text.push_str(&c.text);
            new_text.push_str(&self.text[range.1 as usize..]);
            self.text = new_text;
            self.line_indices = File::make_line_indices(&self.text);
        }

        self.changed = true;
        Ok(())
    }

    fn load_line(&self, line: usize) -> Result<&str, Error> {
        let start = *try_opt_loc!(self.line_indices.get(line));
        let end = *try_opt_loc!(self.line_indices.get(line + 1));

        if (end as usize) <= self.text.len() && start <= end {
            Ok(&self.text[start as usize .. end as usize])
        } else {
            Err(Error::BadLocation)
        }
    }
}

// c is a character offset, returns a byte offset
fn byte_in_str(s: &str, c: usize) -> Option<usize> {
    for (i, (b, _)) in s.char_indices().enumerate() {
        if c == i {
            return Some(b);
        }
    }

    return None;
}

trait FileLoader {
    fn read(file_name: &Path) -> Result<File, Error>;
}

struct RealFileLoader;

impl FileLoader for RealFileLoader {
    fn read(file_name: &Path) -> Result<File, Error> {
        let mut file = match fs::File::open(file_name) {
            Ok(f) => f,
            Err(_) => return Err(Error::Io(Some(file_name.to_owned()), Some(format!("Could not open file: {}", file_name.display())))),
        };
        let mut buf = vec![];
        if let Err(_) = file.read_to_end(&mut buf) {
            return Err(Error::Io(Some(file_name.to_owned()), Some(format!("Could not read file: {}", file_name.display()))));
        }
        let text = String::from_utf8(buf).unwrap();
        Ok(File {
            line_indices: File::make_line_indices(&text),
            text: text,
            changed: false,
        })
    }

}

#[cfg(test)]
mod test {
    use super::{VfsInternal, Change, FileLoader, File, Error};
    use rls_analysis::Span;
    use std::path::{Path, PathBuf};

    struct MockFileLoader;

    impl FileLoader for MockFileLoader {
        fn read(file_name: &Path) -> Result<File, Error> {
            let text = format!("{}\nHello\nWorld\nHello, World!\n", file_name.display());
            Ok(File {
                line_indices: File::make_line_indices(&text),
                text: text,
                changed: false,
            })
        }
    }

    fn make_change() -> Change {
        Change {
            span: Span {
                file_name: "foo".to_owned(),
                line_start: 1,
                line_end: 1,
                column_start: 1,
                column_end: 4,
            },
            text: "foo".to_owned(),
        }
    }

    fn make_change_2() -> Change {
        Change {
            span: Span {
                file_name: "foo".to_owned(),
                line_start: 2,
                line_end: 3,
                column_start: 4,
                column_end: 2,
            },
            text: "aye carumba".to_owned(),
        }
    }

    #[test]
    fn test_has_changes() {
        let vfs = VfsInternal::<MockFileLoader>::new();

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
        let vfs = VfsInternal::<MockFileLoader>::new();
        assert!(vfs.get_cached_files().is_empty());
        vfs.load_file(&Path::new("foo")).unwrap();
        vfs.load_file(&Path::new("bar")).unwrap();
        let files = vfs.get_cached_files();
        assert!(files.len() == 2);
        assert!(files[Path::new("foo")] == "foo\nHello\nWorld\nHello, World!\n");
        assert!(files[Path::new("bar")] == "bar\nHello\nWorld\nHello, World!\n");
    }

    #[test]
    fn test_flush_file() {
        let vfs = VfsInternal::<MockFileLoader>::new();
        // Flushing an uncached-file should succeed.
        vfs.flush_file(&Path::new("foo")).unwrap();
        vfs.load_file(&Path::new("foo")).unwrap();
        vfs.flush_file(&Path::new("foo")).unwrap();
        assert!(vfs.get_cached_files().is_empty());
    }

    #[test]
    fn test_changes() {
        let vfs = VfsInternal::<MockFileLoader>::new();

        vfs.on_changes(&[make_change()]).unwrap();
        let files = vfs.get_cached_files();
        assert!(files.len() == 1);
        assert!(files[&PathBuf::from("foo")] == "foo\nHfooo\nWorld\nHello, World!\n");
        assert!(vfs.load_file(&Path::new("foo")) == Ok("foo\nHfooo\nWorld\nHello, World!\n".to_owned()));
        assert!(vfs.load_file(&Path::new("bar")) == Ok("bar\nHello\nWorld\nHello, World!\n".to_owned()));

        vfs.on_changes(&[make_change_2()]).unwrap();
        let files = vfs.get_cached_files();
        assert!(files.len() == 2);
        assert!(files[&PathBuf::from("foo")] == "foo\nHfooo\nWorlaye carumballo, World!\n");
        assert!(vfs.load_file(&Path::new("foo")) == Ok("foo\nHfooo\nWorlaye carumballo, World!\n".to_owned()));
    }

    // TODO test with wide chars
}
