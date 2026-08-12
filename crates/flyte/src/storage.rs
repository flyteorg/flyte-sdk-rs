//! Blob-store IO by full URI (`s3://…/inputs.pb` etc.), backed by `object_store` —
//! the same crate Python's `obstore` wraps, so ambient credentials (env/IRSA)
//! behave identically in-cluster.

use std::collections::HashMap;
use std::sync::Arc;

use object_store::aws::AmazonS3Builder;
use object_store::azure::MicrosoftAzureBuilder;
use object_store::gcp::GoogleCloudStorageBuilder;
use object_store::local::LocalFileSystem;
use object_store::path::Path as StorePath;
use object_store::{ObjectStore, PutPayload};

use crate::error::Error;

/// Matches Python's MAX_INLINE_IO_BYTES.
pub const MAX_IO_BYTES: usize = 10 * 1024 * 1024;

/// Object storage, with one lazily-built client cached per URI scheme.
///
/// Cloning shares that cache rather than copying it, which is what lets a
/// reusable container build its S3/GCS/Azure clients once and hand the same ones
/// to every action it runs — credential resolution is not cheap, and paying it
/// per action would undo much of the point of a warm container.
#[derive(Clone)]
pub struct Storage {
    stores: Arc<std::sync::Mutex<HashMap<String, Arc<dyn ObjectStore>>>>,
}

impl Default for Storage {
    fn default() -> Self {
        Self::new()
    }
}

impl Storage {
    pub fn new() -> Self {
        Storage {
            stores: Arc::new(std::sync::Mutex::new(HashMap::new())),
        }
    }

    /// Join a path segment onto a URI prefix.
    pub fn join(base: &str, name: &str) -> String {
        format!("{}/{}", base.trim_end_matches('/'), name)
    }

    pub async fn put(&self, uri: &str, data: bytes::Bytes) -> Result<(), Error> {
        if data.len() > MAX_IO_BYTES {
            return Err(Error::Storage(format!(
                "payload for {uri} is {} bytes, exceeds the {MAX_IO_BYTES} byte cap",
                data.len()
            )));
        }
        let (store, path) = self.resolve(uri)?;
        store.put(&path, PutPayload::from_bytes(data)).await?;
        Ok(())
    }

    pub async fn get(&self, uri: &str) -> Result<bytes::Bytes, Error> {
        let (store, path) = self.resolve(uri)?;
        let result = store.get(&path).await?;
        let data = result.bytes().await?;
        if data.len() > MAX_IO_BYTES {
            return Err(Error::Storage(format!(
                "object at {uri} is {} bytes, exceeds the {MAX_IO_BYTES} byte cap",
                data.len()
            )));
        }
        Ok(data)
    }

    fn resolve(&self, uri: &str) -> Result<(Arc<dyn ObjectStore>, StorePath), Error> {
        // Local paths: bare or file:// — one LocalFileSystem store rooted at /.
        if uri.starts_with('/') || uri.starts_with("file://") {
            let fs_path = uri.strip_prefix("file://").unwrap_or(uri);
            let store = self.cached("file://", || Ok(Arc::new(LocalFileSystem::new())))?;
            return Ok((store, StorePath::from(fs_path)));
        }

        let url = url::Url::parse(uri)
            .map_err(|e| Error::Storage(format!("invalid storage uri {uri}: {e}")))?;
        let scheme = url.scheme().to_string();
        let authority = url.host_str().unwrap_or_default().to_string();
        let cache_key = format!("{scheme}://{authority}");
        let object_path = StorePath::from(url.path().trim_start_matches('/'));

        let store = self.cached(&cache_key, || match scheme.as_str() {
            "s3" | "s3a" => {
                let mut builder = AmazonS3Builder::from_env().with_bucket_name(&authority);
                // Devbox/minio: obstore-style endpoint override without full AWS config.
                if let Ok(endpoint) = std::env::var("AWS_ENDPOINT_URL") {
                    builder = builder
                        .with_endpoint(&endpoint)
                        .with_allow_http(endpoint.starts_with("http://"));
                }
                Ok(Arc::new(builder.build()?) as Arc<dyn ObjectStore>)
            }
            "gs" => Ok(Arc::new(
                GoogleCloudStorageBuilder::from_env()
                    .with_bucket_name(&authority)
                    .build()?,
            ) as Arc<dyn ObjectStore>),
            "az" | "abfs" | "abfss" => Ok(Arc::new(
                MicrosoftAzureBuilder::from_env()
                    .with_container_name(&authority)
                    .build()?,
            ) as Arc<dyn ObjectStore>),
            other => Err(Error::Storage(format!(
                "unsupported storage scheme {other:?} in {uri}"
            ))),
        })?;
        Ok((store, object_path))
    }

    fn cached(
        &self,
        key: &str,
        build: impl FnOnce() -> Result<Arc<dyn ObjectStore>, Error>,
    ) -> Result<Arc<dyn ObjectStore>, Error> {
        let mut stores = self.stores.lock().expect("storage cache lock poisoned");
        if let Some(store) = stores.get(key) {
            return Ok(store.clone());
        }
        let store = build()?;
        stores.insert(key.to_string(), store.clone());
        Ok(store)
    }
}
