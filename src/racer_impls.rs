extern crate racer;

use std::io;
use std::path::Path;

use Vfs;

impl racer::FileLoader for Vfs {
    fn load_file(&self, path: &Path) -> io::Result<String> {
        self.0.load_file(path)
            .map_err(|err| io::Error::new(io::ErrorKind::Other, err))
    }
}
