#![feature(proc_macro)]

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

#[cfg(test)]
mod test;

macro_rules! try_opt_loc {
    ($e:expr) => {
        match $e {
            Some(e) => e,
            None => return Err(Error::BadLocation),
        }
    }
}

pub struct Vfs<U = ()>(VfsInternal<RealFileLoader, U>);

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
    /// Not really an error, file is cached but there is no user data for it.
    NoUserDataForFile,
}

impl<U> Vfs<U> {
    /// Creates a new, empty VFS.
    pub fn new() -> Vfs<U> {
        Vfs(VfsInternal::<RealFileLoader, U>::new())
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

    pub fn set_user_data(&self, path: &Path, data: Option<U>) -> Result<(), Error> {
        self.0.set_user_data(path, data)
    }

    pub fn with_user_data<F, R>(&self, path: &Path, f: F) -> R
        where F: FnOnce(Result<&U, Error>) -> R
    {
        self.0.with_user_data(path, f)
    }

    // If f returns NoUserDataForFile, then the user data for the given file is erased.
    pub fn compute_user_data<F>(&self, path: &Path, f: F) -> Result<(), Error>
        where F: FnOnce(&str) -> Result<U, Error>
    {
        self.0.compute_user_data(path, f)
    }
}

struct VfsInternal<T, U> {
    files: Mutex<HashMap<PathBuf, File<U>>>,
    loader: PhantomData<T>,
}

impl<T: FileLoader, U> VfsInternal<T, U> {
    fn new() -> VfsInternal<T, U> {
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
            line_indices: make_line_indices(text),
            changed: true,
            user_data: None,
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

    fn ensure_file(files: &mut HashMap<PathBuf, File<U>>, path: &Path) -> Result<(), Error>{
        if !files.contains_key(path) {
            // TODO we should not hold the lock while we read from disk
            let file = T::read(path)?;
            files.insert(path.to_path_buf(), file);
        }
        Ok(())
    }

    fn write_file(&self, path: &Path) -> Result<(), Error> {
        let mut files = self.files.lock().unwrap();
        match files.get_mut(path) {
            Some(ref mut f) => {
                // TODO drop the lock on files
                T::write(path, f)?;
                f.changed = false;
                Ok(())
            }
            None => Err(Error::FileNotCached),
        }
    }

    pub fn set_user_data(&self, path: &Path, data: Option<U>) -> Result<(), Error> {
        let mut files = self.files.lock().unwrap();
        match files.get_mut(path) {
            Some(ref mut f) => {
                f.user_data = data;
                Ok(())
            }
            None => Err(Error::FileNotCached),
        }
    }

    // Note that f should not be a long-running operation since we hold the lock
    // to the VFS while it runs.
    pub fn with_user_data<F, R>(&self, path: &Path, f: F) -> R
        where F: FnOnce(Result<&U, Error>) -> R
    {
        let files = self.files.lock().unwrap();
        let file = match files.get(path) {
            Some(f) => f,
            None => return f(Err(Error::FileNotCached)),
        };

        f(match file.user_data {
            Some(ref u) => Ok(u),
            None => Err(Error::NoUserDataForFile),
        })
    }

    // Note that f should not be a long-running operation since we hold the lock
    // to the VFS while it runs.
    pub fn compute_user_data<F>(&self, path: &Path, f: F) -> Result<(), Error>
        where F: FnOnce(&str) -> Result<U, Error>
    {
        let mut files = self.files.lock().unwrap();
        match files.get_mut(path) {
            Some(ref mut file) => {
                match f(&file.text) {
                    Ok(u) => {
                        file.user_data = Some(u);
                        Ok(())
                    }
                    Err(Error::NoUserDataForFile) => {
                        file.user_data = None;
                        Ok(())
                    }
                    Err(e) => Err(e),
                }
            },
            None => Err(Error::FileNotCached),
        }
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

fn make_line_indices(text: &str) -> Vec<u32> {
    let mut result = vec![0];
    for (i, b) in text.bytes().enumerate() {
        if b == 0xA {
            result.push((i + 1) as u32);
        }
    }
    result.push(text.len() as u32);
    result
}

struct File<U> {
    // FIXME(https://github.com/jonathandturner/rustls/issues/21) should use a rope.
    text: String,
    line_indices: Vec<u32>,
    changed: bool,
    user_data: Option<U>,
}

impl<U> File<U> {
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
            self.line_indices = make_line_indices(&self.text);
        }

        self.changed = true;
        self.user_data = None;
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
    // We simulate a null-terminated string here because spans are exclusive at
    // the top, and so that index might be outside the length of the string.
    for (i, (b, _)) in s.char_indices().chain(Some((s.len(), '\0')).into_iter()).enumerate() {
        if c == i {
            return Some(b);
        }
    }

    return None;
}

trait FileLoader {
    fn read<U>(file_name: &Path) -> Result<File<U>, Error>;
    fn write<U>(file_name: &Path, file: &File<U>) -> Result<(), Error>;
}

struct RealFileLoader;

impl FileLoader for RealFileLoader {
    fn read<U>(file_name: &Path) -> Result<File<U>, Error> {
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
            line_indices: make_line_indices(&text),
            text: text,
            changed: false,
            user_data: None,
        })
    }

    fn write<U>(file_name: &Path, file: &File<U>) -> Result<(), Error> {
        use std::io::Write;

        macro_rules! try_io {
            ($e:expr) => {
                match $e {
                    Ok(e) => e,
                    Err(e) => return Err(Error::Io(Some(file_name.to_owned()), Some(e.to_string()))),
                }
            }
        }

        let mut out = try_io!(::std::fs::File::create(file_name));
        try_io!(out.write_all(file.text.as_bytes()));
        Ok(())
    }
}
