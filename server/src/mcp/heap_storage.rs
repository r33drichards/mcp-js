use std::path::PathBuf;
use aws_sdk_s3::Client as S3Client;
use aws_sdk_s3::ByteStream;

use aws_config;
use std::sync::Arc;
use async_trait::async_trait;

#[async_trait]
pub trait HeapStorage: Send + Sync + 'static {
    async fn put(&self, name: &str, data: &[u8]) -> Result<(), String>;
    async fn get(&self, name: &str) -> Result<Vec<u8>, String>;
    async fn list(&self) -> Result<Vec<String>, String>;
    async fn delete(&self, name: &str) -> Result<(), String>;
    async fn exists(&self, name: &str) -> Result<bool, String>;
}

#[derive(Clone)]
pub struct FileHeapStorage {
    dir: PathBuf,
}

impl FileHeapStorage {

    // ignore dead_code warning
    #[allow(dead_code)]
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        let dir = dir.into();
        std::fs::create_dir_all(&dir).ok();
        Self { dir }
    }
}

#[async_trait]
impl HeapStorage for FileHeapStorage {
    async fn put(&self, name: &str, data: &[u8]) -> Result<(), String> {
        let path = self.dir.join(name);
        std::fs::write(path, data).map_err(|e| e.to_string())
    }
    async fn get(&self, name: &str) -> Result<Vec<u8>, String> {
        let path = self.dir.join(name);
        std::fs::read(path).map_err(|e| e.to_string())
    }
    async fn list(&self) -> Result<Vec<String>, String> {
        let entries = std::fs::read_dir(&self.dir).map_err(|e| e.to_string())?;
        let mut names = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| e.to_string())?;
            if let Some(name) = entry.file_name().to_str() {
                names.push(name.to_string());
            }
        }
        Ok(names)
    }
    async fn delete(&self, name: &str) -> Result<(), String> {
        let path = self.dir.join(name);
        std::fs::remove_file(path).map_err(|e| e.to_string())
    }
    async fn exists(&self, name: &str) -> Result<bool, String> {
        let path = self.dir.join(name);
        Ok(path.exists())
    }
}

#[derive(Clone)]
pub struct S3HeapStorage {
    bucket: String,
    client: Arc<S3Client>,
}

impl S3HeapStorage {
    pub async fn new(bucket: impl Into<String>) -> Self {
        let config = aws_config::load_from_env().await;
        let client = S3Client::new(&config);
        Self {
            bucket: bucket.into(),
            client: Arc::new(client),
        }
    }

    async fn put_blocking(&self, name: &str, data: &[u8]) -> Result<(), String> {
        let client = self.client.clone();
        let bucket = self.bucket.clone();
        let name = name.to_string();
        let data = data.to_vec();
        client
            .put_object()
            .bucket(bucket)
            .key(name)
            .body(ByteStream::from(data))
            .send()
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    async fn get_blocking(&self, name: &str) -> Result<Vec<u8>, String> {
        let client = self.client.clone();
        let bucket = self.bucket.clone();
        let name = name.to_string();
        let output = client
            .get_object()
            .bucket(bucket)
            .key(name)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        let data = output.body.collect().await.map_err(|e| e.to_string())?;
        Ok(data.into_bytes().to_vec())
    }

    async fn list_blocking(&self) -> Result<Vec<String>, String> {
        let client = self.client.clone();
        let bucket = self.bucket.clone();
        let output = client
            .list_objects_v2()
            .bucket(bucket)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        let names = output
            .contents
            .unwrap_or_default()
            .iter()
            .filter_map(|obj| obj.key.as_ref().map(|k| k.to_string()))
            .collect();
        Ok(names)
    }

    async fn delete_blocking(&self, name: &str) -> Result<(), String> {
        let client = self.client.clone();
        let bucket = self.bucket.clone();
        let name = name.to_string();
        client
            .delete_object()
            .bucket(bucket)
            .key(name)
            .send()
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    async fn exists_blocking(&self, name: &str) -> Result<bool, String> {
        let client = self.client.clone();
        let bucket = self.bucket.clone();
        let name = name.to_string();
        match client
            .head_object()
            .bucket(bucket)
            .key(name)
            .send()
            .await
        {
            Ok(_) => Ok(true),
            Err(_) => Ok(false),
        }
    }
}

#[async_trait]
impl HeapStorage for S3HeapStorage {
    async fn put(&self, name: &str, data: &[u8]) -> Result<(), String> {
        self.put_blocking(name, data).await
    }
    async fn get(&self, name: &str) -> Result<Vec<u8>, String> {
        self.get_blocking(name).await
    }
    async fn list(&self) -> Result<Vec<String>, String> {
        self.list_blocking().await
    }
    async fn delete(&self, name: &str) -> Result<(), String> {
        self.delete_blocking(name).await
    }
    async fn exists(&self, name: &str) -> Result<bool, String> {
        self.exists_blocking(name).await
    }
}

#[derive(Clone)]
pub struct MultiHeapStorage {
    file: Option<FileHeapStorage>,
    s3: Option<S3HeapStorage>,
}

impl MultiHeapStorage {
    pub fn new(file: Option<FileHeapStorage>, s3: Option<S3HeapStorage>) -> Self {
        Self { file, s3 }
    }

    fn parse_uri<'a>(&self, uri: &'a str) -> Result<(&'a str, &'a str), String> {
        if let Some(path) = uri.strip_prefix("file://") {
            Ok(("file", path))
        } else if let Some(key) = uri.strip_prefix("s3://") {
            Ok(("s3", key))
        } else {
            Err(format!("Invalid URI scheme: {}. Must be file:// or s3://", uri))
        }
    }

    async fn get_by_uri(&self, uri: &str) -> Result<Vec<u8>, String> {
        let (scheme, path) = self.parse_uri(uri)?;
        match scheme {
            "file" => {
                let storage = self.file.as_ref().ok_or("File storage not configured")?;
                storage.get(path).await
            }
            "s3" => {
                let storage = self.s3.as_ref().ok_or("S3 storage not configured")?;
                storage.get(path).await
            }
            _ => Err(format!("Unsupported scheme: {}", scheme)),
        }
    }

    async fn put_by_uri(&self, uri: &str, data: &[u8]) -> Result<(), String> {
        let (scheme, path) = self.parse_uri(uri)?;
        match scheme {
            "file" => {
                let storage = self.file.as_ref().ok_or("File storage not configured")?;
                storage.put(path, data).await
            }
            "s3" => {
                let storage = self.s3.as_ref().ok_or("S3 storage not configured")?;
                storage.put(path, data).await
            }
            _ => Err(format!("Unsupported scheme: {}", scheme)),
        }
    }

    async fn delete_by_uri(&self, uri: &str) -> Result<(), String> {
        let (scheme, path) = self.parse_uri(uri)?;
        match scheme {
            "file" => {
                let storage = self.file.as_ref().ok_or("File storage not configured")?;
                storage.delete(path).await
            }
            "s3" => {
                let storage = self.s3.as_ref().ok_or("S3 storage not configured")?;
                storage.delete(path).await
            }
            _ => Err(format!("Unsupported scheme: {}", scheme)),
        }
    }

    async fn exists_by_uri(&self, uri: &str) -> Result<bool, String> {
        let (scheme, path) = self.parse_uri(uri)?;
        match scheme {
            "file" => {
                let storage = self.file.as_ref().ok_or("File storage not configured")?;
                storage.exists(path).await
            }
            "s3" => {
                let storage = self.s3.as_ref().ok_or("S3 storage not configured")?;
                storage.exists(path).await
            }
            _ => Err(format!("Unsupported scheme: {}", scheme)),
        }
    }

    pub async fn list_all(&self) -> Result<Vec<String>, String> {
        let mut all_heaps = Vec::new();

        if let Some(file_storage) = &self.file {
            let file_heaps = file_storage.list().await?;
            all_heaps.extend(file_heaps.into_iter().map(|name| format!("file://{}", name)));
        }

        if let Some(s3_storage) = &self.s3 {
            let s3_heaps = s3_storage.list().await?;
            all_heaps.extend(s3_heaps.into_iter().map(|name| format!("s3://{}", name)));
        }

        Ok(all_heaps)
    }
}

#[derive(Clone)]
pub enum AnyHeapStorage {
    #[allow(dead_code)]
    File(FileHeapStorage),
    S3(S3HeapStorage),
    Multi(MultiHeapStorage),
}




impl AnyHeapStorage {
    // URI-based methods for Multi storage
    pub async fn get_by_uri(&self, uri: &str) -> Result<Vec<u8>, String> {
        match self {
            AnyHeapStorage::Multi(inner) => inner.get_by_uri(uri).await,
            AnyHeapStorage::File(inner) => {
                let path = uri.strip_prefix("file://").ok_or("URI must start with file://")?;
                inner.get(path).await
            }
            AnyHeapStorage::S3(inner) => {
                let key = uri.strip_prefix("s3://").ok_or("URI must start with s3://")?;
                inner.get(key).await
            }
        }
    }

    pub async fn put_by_uri(&self, uri: &str, data: &[u8]) -> Result<(), String> {
        match self {
            AnyHeapStorage::Multi(inner) => inner.put_by_uri(uri, data).await,
            AnyHeapStorage::File(inner) => {
                let path = uri.strip_prefix("file://").ok_or("URI must start with file://")?;
                inner.put(path, data).await
            }
            AnyHeapStorage::S3(inner) => {
                let key = uri.strip_prefix("s3://").ok_or("URI must start with s3://")?;
                inner.put(key, data).await
            }
        }
    }

    pub async fn delete_by_uri(&self, uri: &str) -> Result<(), String> {
        match self {
            AnyHeapStorage::Multi(inner) => inner.delete_by_uri(uri).await,
            AnyHeapStorage::File(inner) => {
                let path = uri.strip_prefix("file://").ok_or("URI must start with file://")?;
                inner.delete(path).await
            }
            AnyHeapStorage::S3(inner) => {
                let key = uri.strip_prefix("s3://").ok_or("URI must start with s3://")?;
                inner.delete(key).await
            }
        }
    }

    pub async fn exists_by_uri(&self, uri: &str) -> Result<bool, String> {
        match self {
            AnyHeapStorage::Multi(inner) => inner.exists_by_uri(uri).await,
            AnyHeapStorage::File(inner) => {
                let path = uri.strip_prefix("file://").ok_or("URI must start with file://")?;
                inner.exists(path).await
            }
            AnyHeapStorage::S3(inner) => {
                let key = uri.strip_prefix("s3://").ok_or("URI must start with s3://")?;
                inner.exists(key).await
            }
        }
    }

    pub async fn list_all(&self) -> Result<Vec<String>, String> {
        match self {
            AnyHeapStorage::Multi(inner) => inner.list_all().await,
            AnyHeapStorage::File(inner) => {
                let names = inner.list().await?;
                Ok(names.into_iter().map(|name| format!("file://{}", name)).collect())
            }
            AnyHeapStorage::S3(inner) => {
                let names = inner.list().await?;
                Ok(names.into_iter().map(|name| format!("s3://{}", name)).collect())
            }
        }
    }
}

#[async_trait::async_trait]
impl HeapStorage for AnyHeapStorage {
    async fn put(&self, name: &str, data: &[u8]) -> Result<(), String> {
        match self {
            AnyHeapStorage::File(inner) => inner.put(name, data).await,
            AnyHeapStorage::S3(inner) => inner.put(name, data).await,
            AnyHeapStorage::Multi(_) => Err("Use put_by_uri for Multi storage".to_string()),
        }
    }
    async fn get(&self, name: &str) -> Result<Vec<u8>, String> {
        match self {
            AnyHeapStorage::File(inner) => inner.get(name).await,
            AnyHeapStorage::S3(inner) => inner.get(name).await,
            AnyHeapStorage::Multi(_) => Err("Use get_by_uri for Multi storage".to_string()),
        }
    }
    async fn list(&self) -> Result<Vec<String>, String> {
        match self {
            AnyHeapStorage::File(inner) => inner.list().await,
            AnyHeapStorage::S3(inner) => inner.list().await,
            AnyHeapStorage::Multi(_) => Err("Use list_all for Multi storage".to_string()),
        }
    }
    async fn delete(&self, name: &str) -> Result<(), String> {
        match self {
            AnyHeapStorage::File(inner) => inner.delete(name).await,
            AnyHeapStorage::S3(inner) => inner.delete(name).await,
            AnyHeapStorage::Multi(_) => Err("Use delete_by_uri for Multi storage".to_string()),
        }
    }
    async fn exists(&self, name: &str) -> Result<bool, String> {
        match self {
            AnyHeapStorage::File(inner) => inner.exists(name).await,
            AnyHeapStorage::S3(inner) => inner.exists(name).await,
            AnyHeapStorage::Multi(_) => Err("Use exists_by_uri for Multi storage".to_string()),
        }
    }
} 