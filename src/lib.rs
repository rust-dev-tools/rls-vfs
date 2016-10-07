#![feature(rustc_macro)]

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

// My kingdom for ?/Option support
macro_rules! try_opt {
    ($e:expr) => {
        match $e {
            Some(e) => e,
            None => return None,
        }
    }
}

pub struct Vfs(VfsInternal<RealFileLoader>);

#[derive(Debug, Deserialize, Serialize)]
pub struct Change {
    pub span: Span,
    pub text: String,
}

impl Vfs {
    pub fn new() -> Vfs {
        Vfs(VfsInternal::<RealFileLoader>::new())
    }

    // TODO should take a Path
    pub fn on_save(&self, file_name: &str) {
        self.0.on_save(file_name)
    }

    pub fn on_change(&self, changes: &[Change]) {
        self.0.on_change(changes)
    }

    pub fn get_changed_files(&self) -> HashMap<PathBuf, String> {
        self.0.get_changed_files()
    }

    pub fn get_file_changes(&self, path: &Path) -> Option<String> {
        self.0.get_file_changes(path)
    }

    pub fn has_changes(&self) -> bool {
        self.0.has_changes()
    }

    pub fn set_file(&self, path: &Path, text: &str) {
        self.0.set_file(path, text)
    }

    pub fn get_line(&self, path: &Path, line: usize) -> Option<String> {
        self.0.get_line(path, line)
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
}

impl<T: FileLoader> VfsInternal<T> {
    fn new() -> VfsInternal<T> {
        VfsInternal {
            files: Mutex::new(HashMap::new()),
            loader: PhantomData,
        }
    }

    fn on_save(&self, file_name: &str) {
        let mut files = self.files.lock().unwrap();
        files.remove(Path::new(file_name));
    }

    fn on_change(&self, changes: &[Change]) {
        for (file_name, changes) in VfsInternal::<T>::coalesce_changes(changes) {
            let path = Path::new(file_name);
            {
                let mut files = self.files.lock().unwrap();
                if let Some(file) = files.get_mut(Path::new(path)) {
                    file.make_change(&changes);
                    return;
                }
            }

            let mut file = T::read(Path::new(path)).unwrap();
            file.make_change(&changes);

            let mut files = self.files.lock().unwrap();
            files.insert(path.to_path_buf(), file);
        }
    }

    fn set_file(&self, path: &Path, text: &str) {
        let file = File {
            text: text.to_owned(),
            line_indices: File::make_line_indices(text),
        };

        let mut files = self.files.lock().unwrap();
        files.insert(path.to_owned(), file);
    }

    fn get_changed_files(&self) -> HashMap<PathBuf, String> {
        let files = self.files.lock().unwrap();
        files.iter().map(|(p, f)| (p.clone(), f.text.clone())).collect()
    }

    fn get_file_changes(&self, path: &Path) -> Option<String> {
        let files = self.files.lock().unwrap();
        files.get(path).map(|f| f.text.clone())
    }

    fn has_changes(&self) -> bool {
        let files = self.files.lock().unwrap();
        // TODO bogus - could contain unchanged files
        files.len() != 0        
    }

    fn coalesce_changes<'a>(changes: &'a [Change]) -> HashMap<&'a str, Vec<&'a Change>> {
        // Note that for any given file, we preserve the order of the changes.
        let mut result = HashMap::new();
        for c in changes {
            result.entry(&*c.span.file_name).or_insert(vec![]).push(c);
        }
        result
    }

    fn get_line(&self, path: &Path, line: usize) -> Option<String> {
        let mut files = self.files.lock().unwrap();
        Self::ensure_file(&mut files, path);

        files.get(path).and_then(|file| file.get_line(line).map(|s| s.to_owned()))
    }

    fn ensure_file(files: &mut HashMap<PathBuf, File>, path: &Path) {
        if !files.contains_key(path) {
            let file = T::read(path).unwrap();
            files.insert(path.to_path_buf(), file);
        }
    }
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

    fn make_change(&mut self, changes: &[&Change]) {
        for c in changes {
            let range = {
                let first_line = self.get_line(c.span.line_start).unwrap();
                let last_line = self.get_line(c.span.line_end).unwrap();

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
    }

    fn get_line(&self, line: usize) -> Option<&str> {
        let start = *try_opt!(self.line_indices.get(line));
        let end = *try_opt!(self.line_indices.get(line + 1));

        if (end as usize) <= self.text.len() && start <= end {
            Some(&self.text[start as usize .. end as usize])
        } else {
            None
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
    fn read(file_name: &Path) -> Result<File, String>;
}

struct RealFileLoader;

impl FileLoader for RealFileLoader {
    fn read(file_name: &Path) -> Result<File, String> {
        let mut file = match fs::File::open(file_name) {
            Ok(f) => f,
            Err(_) => return Err(format!("Could not open file: {}", file_name.display())),
        };
        let mut buf = vec![];
        if let Err(_) = file.read_to_end(&mut buf) {
            return Err(format!("Could not read file: {}", file_name.display()));
        }
        let text = String::from_utf8(buf).unwrap();
        Ok(File {
            line_indices: File::make_line_indices(&text),
            text: text,
        })
    }

}

#[cfg(test)]
mod test {
    use super::{VfsInternal, Change, FileLoader, File};
    use rls_analysis::Span;
    use std::path::{Path, PathBuf};

    struct MockFileLoader;

    impl FileLoader for MockFileLoader {
        fn read(file_name: &Path) -> Result<File, String> {
            let text = format!("{}\nHello\nWorld\nHello, World!\n", file_name.display());
            Ok(File {
                line_indices: File::make_line_indices(&text),
                text: text,
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
        vfs.on_change(&[make_change()]);
        assert!(vfs.has_changes());
        vfs.on_save("bar");
        assert!(vfs.has_changes());
        vfs.on_save("foo");
        assert!(!vfs.has_changes());
        assert!(vfs.get_changed_files().is_empty());
    }

    #[test]
    fn test_changes() {
        let vfs = VfsInternal::<MockFileLoader>::new();

        vfs.on_change(&[make_change()]);
        let changes = vfs.get_changed_files();
        assert!(changes.len() == 1);
        assert!(changes[&PathBuf::from("foo")] == "foo\nHfooo\nWorld\nHello, World!\n");

        vfs.on_change(&[make_change_2()]);
        let changes = vfs.get_changed_files();
        assert!(changes.len() == 1);
        assert!(changes[&PathBuf::from("foo")] == "foo\nHfooo\nWorlaye carumballo, World!\n");
    }

    // TODO test with wide chars
}
