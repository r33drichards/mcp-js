use std::path::PathBuf;

pub trait HeapStorage: Send + Sync + 'static {
    fn put(&self, name: &str, data: &[u8]) -> Result<(), String>;
    fn get(&self, name: &str) -> Result<Vec<u8>, String>;
}

pub struct FileHeapStorage {
    dir: PathBuf,
}

impl FileHeapStorage {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        let dir = dir.into();
        std::fs::create_dir_all(&dir).ok();
        Self { dir }
    }
}

impl HeapStorage for FileHeapStorage {
    fn put(&self, name: &str, data: &[u8]) -> Result<(), String> {
        let path = self.dir.join(name);
        std::fs::write(path, data).map_err(|e| e.to_string())
    }
    fn get(&self, name: &str) -> Result<Vec<u8>, String> {
        let path = self.dir.join(name);
        std::fs::read(path).map_err(|e| e.to_string())
    }
} 