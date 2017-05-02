#![feature(type_ascription)]

extern crate rls_span as span;

use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::io::Read;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[cfg(feature = "racer-impls")]
mod racer_impls;

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

type Span = span::Span<span::ZeroIndexed>;

#[derive(Debug)]
pub enum Change {
    /// Create an in-memory image of the file.
    AddFile {
        file: PathBuf,
        text: String,
    },
    /// Changes in-memory contents of the previously added file.
    ReplaceText {
        /// Span of the text to be replaced defined in col/row terms.
        span: Span,
        /// Length in chars of the text to be replaced. If present,
        /// used to calculate replacement range instead of
        /// span's row_end/col_end fields. Needed for editors that
        /// can't properly calculate the latter fields.
        /// Span's row_start/col_start are still assumed valid.
        len: Option<u64>,
        /// Text to replace specified text range with.
        text: String,
    },
}

impl Change {
    fn file(&self) -> &Path {
        match *self {
            Change::AddFile { ref file, .. } => file.as_ref(),
            Change::ReplaceText { ref span, .. } => span.file.as_ref(),
        }
    }
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

impl ::std::error::Error for Error {
    fn description(&self) -> &str {
        match *self {
            Error::OutOfSync(ref _path_buf) => "file out of sync with filesystem",
            Error::Io(ref _path_buf, ref _message) => "io::Error reading or writing path",
            Error::UncommittedChanges(ref _path_buf) => {
                "changes exist which have not been written to disk"
            },
            Error::BadLocation => "client specified location not existing within a file",
            Error::FileNotCached => "requested file was not cached in the VFS",
            Error::NoUserDataForFile => "file is cached but there is no user data for it",
        }
    }
}

impl Into<String> for Error {
    fn into(self) -> String {
        ::std::error::Error::description(&self).to_owned()
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            Error::OutOfSync(ref path_buf) => {
                write!(f, "file {} out of sync with filesystem", path_buf.display())
            },
            Error::UncommittedChanges(ref path_buf) => {
                write!(f, "{} has uncommitted changes", path_buf.display())
            },
            Error::BadLocation
            | Error::FileNotCached
            | Error::NoUserDataForFile
            | Error::Io(_, _)
            => {
                f.write_str(::std::error::Error::description(self))
            }
        }
    }
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

    pub fn load_line(&self, path: &Path, line: span::Row<span::ZeroIndexed>) -> Result<String, Error> {
        self.0.load_line(path, line)
    }

    pub fn write_file(&self, path: &Path) -> Result<(), Error> {
        self.0.write_file(path)
    }

    pub fn set_user_data(&self, path: &Path, data: Option<U>) -> Result<(), Error> {
        self.0.set_user_data(path, data)
    }

    // If f returns NoUserDataForFile, then the user data for the given file is erased.
    pub fn with_user_data<F, R>(&self, path: &Path, f: F) -> Result<R, Error>
        where F: FnOnce(Result<(&str, &mut U), Error>) -> Result<R, Error>
    {
        self.0.with_user_data(path, f)
    }

    // If f returns NoUserDataForFile, then the user data for the given file is erased.
    pub fn ensure_user_data<F>(&self, path: &Path, f: F) -> Result<(), Error>
        where F: FnOnce(&str) -> Result<U, Error>
    {
        self.0.ensure_user_data(path, f)
    }

    pub fn clear(&self) {
        self.0.clear()
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

    fn clear(&self) {
        let mut files = self.files.lock().unwrap();
        *files = HashMap::new();
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

            // FIXME(#11): if the first change is `Add`, we should avoid
            // loading the file. If the first change is not `Add`, then
            // this is subtly broken, because we can't guarantee that the
            // edits are intended to be applied to the version of the file
            // we read from disk. That is, the on disk contents might have
            // changed after the edit request.
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

    fn load_line(&self, path: &Path, line: span::Row<span::ZeroIndexed>) -> Result<String, Error> {
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
    pub fn with_user_data<F, R>(&self, path: &Path, f: F) -> Result<R, Error>
        where F: FnOnce(Result<(&str, &mut U), Error>) -> Result<R, Error>
    {
        let mut files = self.files.lock().unwrap();
        let file = match files.get_mut(path) {
            Some(f) => f,
            None => return f(Err(Error::FileNotCached)),
        };

        let result = f(match file.user_data {
            Some(ref mut u) => Ok((&file.text, u)),
            None => Err(Error::NoUserDataForFile),
        });

        if let Err(Error::NoUserDataForFile) = result {
            file.user_data = None;
        }

        result
    }

    pub fn ensure_user_data<F>(&self, path: &Path, f: F) -> Result<(), Error>
        where F: FnOnce(&str) -> Result<U, Error>
    {
        let mut files = self.files.lock().unwrap();
        match files.get_mut(path) {
            Some(ref mut file) => {
                if let None = file.user_data {
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
                } else {
                    Ok(())
                }
            },
            None => Err(Error::FileNotCached),
        }
    }
}

fn coalesce_changes<'a>(changes: &'a [Change]) -> HashMap<&'a Path, Vec<&'a Change>> {
    // Note that for any given file, we preserve the order of the changes.
    let mut result = HashMap::new();
    for c in changes {
        result.entry(&*c.file()).or_insert(vec![]).push(c);
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
            let new_text = match **c {
                Change::ReplaceText { ref span, ref len, ref text } => {
                    let range = {
                        let first_line = self.load_line(span.range.row_start).unwrap();
                        let byte_start = self.line_indices[span.range.row_start.0 as usize] +
                            byte_in_str(first_line, span.range.col_start).unwrap() as u32;

                        let byte_end = if let &Some(len) = len {
                            // if `len` exists, the replaced portion of text
                            // is `len` chars starting from row_start/col_start.
                            byte_start + byte_in_str(
                                &self.text[byte_start as usize..],
                                span::Column::new_zero_indexed(len as u32)
                            ).unwrap() as u32
                        } else {
                            // if no `len`, fall back to using row_end/col_end
                            // for determining the tail end of replaced text.
                            let last_line = self.load_line(span.range.row_end).unwrap();
                            self.line_indices[span.range.row_end.0 as usize] +
                                byte_in_str(last_line, span.range.col_end).unwrap() as u32
                        };

                        (byte_start, byte_end)
                    };
                    let mut new_text = self.text[..range.0 as usize].to_owned();
                    new_text.push_str(text);
                    new_text.push_str(&self.text[range.1 as usize..]);
                    new_text
                }
                Change::AddFile { file: _, ref text } => text.to_owned()
            };

            self.text = new_text;
            self.line_indices = make_line_indices(&self.text);
        }

        self.changed = true;
        self.user_data = None;
        Ok(())
    }

    fn load_line(&self, line: span::Row<span::ZeroIndexed>) -> Result<&str, Error> {
        let start = *try_opt_loc!(self.line_indices.get(line.0 as usize));
        let end = *try_opt_loc!(self.line_indices.get(line.0 as usize + 1));

        if (end as usize) <= self.text.len() && start <= end {
            Ok(&self.text[start as usize .. end as usize])
        } else {
            Err(Error::BadLocation)
        }
    }
}

// c is a character offset, returns a byte offset
fn byte_in_str(s: &str, c: span::Column<span::ZeroIndexed>) -> Option<usize> {
    // We simulate a null-terminated string here because spans are exclusive at
    // the top, and so that index might be outside the length of the string.
    for (i, (b, _)) in s.char_indices().chain(Some((s.len(), '\0')).into_iter()).enumerate() {
        if c.0 as usize == i {
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
        let text = String::from_utf8(buf).map_err(|e| Error::Io(Some(file_name.to_owned()), Some(::std::error::Error::description(&e).to_owned())))?;
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
