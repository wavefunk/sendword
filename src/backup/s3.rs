use s3::bucket::Bucket;
use s3::creds::Credentials;
use s3::region::Region;

use crate::config::BackupConfig;
use crate::error::DbError;

/// A summary of an object stored in S3.
#[derive(Debug, Clone)]
pub struct BackupEntry {
    pub key: String,
    pub size: u64,
    pub last_modified: String,
}

/// Error type for S3 operations.
#[derive(Debug, thiserror::Error)]
pub enum S3Error {
    #[error("S3 error: {0}")]
    S3(#[from] s3::error::S3Error),
    #[error("credentials error: {0}")]
    Credentials(#[from] s3::creds::error::CredentialsError),
}

/// Thin wrapper around a `rust-s3` `Bucket` for testability and config-driven construction.
pub struct S3Client {
    bucket: Box<Bucket>,
    prefix: String,
}

impl S3Client {
    /// Construct a client from the backup config.
    pub fn new(config: &BackupConfig) -> Result<Self, S3Error> {
        let region = if config.endpoint.is_empty() {
            config.region.parse::<Region>().unwrap_or(Region::UsEast1)
        } else {
            Region::Custom {
                region: if config.region.is_empty() {
                    "us-east-1".into()
                } else {
                    config.region.clone()
                },
                endpoint: config.endpoint.clone(),
            }
        };

        let credentials = Credentials::new(
            Some(&config.access_key),
            Some(&config.secret_key),
            None,
            None,
            None,
        )?;

        let mut bucket = Bucket::new(&config.bucket, region, credentials)?;
        // Use path-style URLs for compatibility with Minio and other S3-compatible stores.
        bucket.set_path_style();

        Ok(Self {
            bucket,
            prefix: config.prefix.clone(),
        })
    }

    /// Build the full object key by prepending the configured prefix.
    fn key(&self, name: &str) -> String {
        if self.prefix.is_empty() {
            name.to_owned()
        } else {
            format!("{}/{}", self.prefix.trim_end_matches('/'), name)
        }
    }

    /// Upload `data` to `name` (relative to prefix).
    pub async fn put(&self, name: &str, data: &[u8]) -> Result<(), S3Error> {
        let key = self.key(name);
        self.bucket.put_object(key, data).await?;
        Ok(())
    }

    /// Download object `name` and return its bytes.
    pub async fn get(&self, name: &str) -> Result<Vec<u8>, S3Error> {
        let key = self.key(name);
        let resp = self.bucket.get_object(key).await?;
        Ok(resp.to_vec())
    }

    /// List all objects under the configured prefix.
    pub async fn list(&self) -> Result<Vec<BackupEntry>, S3Error> {
        let prefix = if self.prefix.is_empty() {
            String::new()
        } else {
            format!("{}/", self.prefix.trim_end_matches('/'))
        };

        let pages = self.bucket.list(prefix.clone(), None).await?;
        let mut entries = Vec::new();
        for page in pages {
            for obj in page.contents {
                // Strip the common prefix from the key for display
                let display_key = if !prefix.is_empty() && obj.key.starts_with(&prefix) {
                    obj.key[prefix.len()..].to_owned()
                } else {
                    obj.key.clone()
                };
                entries.push(BackupEntry {
                    key: display_key,
                    size: obj.size,
                    last_modified: obj.last_modified,
                });
            }
        }

        Ok(entries)
    }

    /// Delete object `name`.
    pub async fn delete(&self, name: &str) -> Result<(), S3Error> {
        let key = self.key(name);
        self.bucket.delete_object(key).await?;
        Ok(())
    }
}
