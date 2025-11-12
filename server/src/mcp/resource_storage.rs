use async_trait::async_trait;
use std::path::PathBuf;

/// Trait for resource storage operations
#[async_trait]
pub trait ResourceStorage: Send + Sync + 'static {
    async fn read(&self, uri: &str) -> Result<String, String>;
    async fn list(&self, uri: &str) -> Result<Vec<String>, String>;
    async fn write(&self, uri: &str, content: &str) -> Result<(), String>;
    async fn delete(&self, uri: &str) -> Result<(), String>;
}

/// File-based resource storage implementation
pub struct FileResourceStorage {
    base_path: PathBuf,
}

impl FileResourceStorage {
    pub fn new(base_path: PathBuf) -> Self {
        Self { base_path }
    }

    fn resolve_uri(&self, uri: &str) -> Result<PathBuf, String> {
        // Parse URI like "file:///path/to/resource" or just "/path/to/resource"
        let path_str = if uri.starts_with("file://") {
            &uri[7..]
        } else {
            uri
        };

        // Remove leading slash for relative path resolution
        let path_str = path_str.trim_start_matches('/');

        let full_path = self.base_path.join(path_str);

        // Security: ensure path is within base_path
        let canonical_base = self.base_path.canonicalize()
            .map_err(|e| format!("Failed to canonicalize base path: {}", e))?;

        // For paths that don't exist yet (for write operations), check parent
        let path_to_check = if full_path.exists() {
            full_path.canonicalize()
                .map_err(|e| format!("Failed to canonicalize path: {}", e))?
        } else {
            let parent = full_path.parent()
                .ok_or_else(|| "Invalid path: no parent directory".to_string())?;
            if parent.exists() {
                parent.canonicalize()
                    .map_err(|e| format!("Failed to canonicalize parent: {}", e))?
                    .join(full_path.file_name().unwrap())
            } else {
                // Parent doesn't exist, use the full path as-is for checking
                full_path.clone()
            }
        };

        if !path_to_check.starts_with(&canonical_base) {
            return Err("Path outside base directory".to_string());
        }

        Ok(full_path)
    }
}

#[async_trait]
impl ResourceStorage for FileResourceStorage {
    async fn read(&self, uri: &str) -> Result<String, String> {
        let path = self.resolve_uri(uri)?;
        tokio::fs::read_to_string(path)
            .await
            .map_err(|e| format!("Failed to read file: {}", e))
    }

    async fn list(&self, uri: &str) -> Result<Vec<String>, String> {
        let path = self.resolve_uri(uri)?;

        if !path.exists() {
            return Err("Path does not exist".to_string());
        }

        if !path.is_dir() {
            return Err("Path is not a directory".to_string());
        }

        let mut entries = tokio::fs::read_dir(path)
            .await
            .map_err(|e| format!("Failed to read directory: {}", e))?;

        let mut results = Vec::new();
        while let Some(entry) = entries.next_entry()
            .await
            .map_err(|e| format!("Failed to read directory entry: {}", e))? {
            if let Some(name) = entry.file_name().to_str() {
                results.push(name.to_string());
            }
        }

        Ok(results)
    }

    async fn write(&self, uri: &str, content: &str) -> Result<(), String> {
        let path = self.resolve_uri(uri)?;

        // Create parent directories if they don't exist
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| format!("Failed to create parent directories: {}", e))?;
        }

        tokio::fs::write(path, content)
            .await
            .map_err(|e| format!("Failed to write file: {}", e))
    }

    async fn delete(&self, uri: &str) -> Result<(), String> {
        let path = self.resolve_uri(uri)?;

        if !path.exists() {
            return Err("Path does not exist".to_string());
        }

        if path.is_dir() {
            tokio::fs::remove_dir_all(path)
                .await
                .map_err(|e| format!("Failed to delete directory: {}", e))
        } else {
            tokio::fs::remove_file(path)
                .await
                .map_err(|e| format!("Failed to delete file: {}", e))
        }
    }
}
