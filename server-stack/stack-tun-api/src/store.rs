use crate::types::PersistRoot;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Clone)]
pub struct ConfigStore {
    path: PathBuf,
    inner: Arc<RwLock<PersistRoot>>,
}

impl ConfigStore {
    pub async fn load(path: PathBuf) -> Result<Self, String> {
        let data = if path.exists() {
            let s = tokio::fs::read_to_string(&path)
                .await
                .map_err(|e| e.to_string())?;
            serde_json::from_str(&s).unwrap_or_default()
        } else {
            PersistRoot::default()
        };
        Ok(Self {
            path,
            inner: Arc::new(RwLock::new(data)),
        })
    }

    pub async fn save_to_disk(&self) -> Result<(), String> {
        let g = self.inner.read().await;
        if let Some(p) = self.path.parent() {
            tokio::fs::create_dir_all(p)
                .await
                .map_err(|e| e.to_string())?;
        }
        let body = serde_json::to_string_pretty(&*g).map_err(|e| e.to_string())?;
        tokio::fs::write(&self.path, body)
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub async fn read_root(&self) -> PersistRoot {
        self.inner.read().await.clone()
    }

    pub async fn replace_root(&self, root: PersistRoot) {
        let mut g = self.inner.write().await;
        *g = root;
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}
