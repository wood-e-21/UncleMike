use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct WorkspacePaths {
    pub root: PathBuf,
    pub mike_dir: PathBuf,
    pub runtime_dir: PathBuf,
    pub db_path: PathBuf,
    pub matters_dir: PathBuf,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceJson {
    schema_version: u32,
    id: String,
    created_at: String,
    mike_version_first: String,
}

impl WorkspacePaths {
    pub fn from_env() -> Result<Self> {
        let raw = std::env::var("WORKSPACE_PATH")
            .context("WORKSPACE_PATH is required; Electron must choose a workspace before starting the backend")?;
        Self::new(raw)
    }

    pub fn new(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        if root.as_os_str().is_empty() {
            anyhow::bail!("WORKSPACE_PATH cannot be empty");
        }
        let root = if root.is_absolute() {
            root
        } else {
            std::env::current_dir()?.join(root)
        };
        let mike_dir = root.join(".mike");
        let runtime_dir = mike_dir.join("runtime");
        let matters_dir = root.join("matters");
        let db_path = mike_dir.join("mike.db");
        let paths = Self {
            root,
            mike_dir,
            runtime_dir,
            db_path,
            matters_dir,
        };
        paths.ensure_layout()?;
        Ok(paths)
    }

    pub fn ensure_layout(&self) -> Result<()> {
        std::fs::create_dir_all(&self.mike_dir)?;
        std::fs::create_dir_all(&self.runtime_dir)?;
        std::fs::create_dir_all(&self.matters_dir)?;
        std::fs::create_dir_all(self.matters_dir.join("_unfiled").join("items"))?;
        std::fs::create_dir_all(self.matters_dir.join("_unfiled").join("attachments"))?;
        self.ensure_workspace_json()?;
        Ok(())
    }

    fn ensure_workspace_json(&self) -> Result<()> {
        let path = self.mike_dir.join("workspace.json");
        if path.exists() {
            return Ok(());
        }
        let doc = WorkspaceJson {
            schema_version: 1,
            id: uuid::Uuid::new_v4().to_string(),
            created_at: Utc::now().to_rfc3339(),
            mike_version_first: env!("CARGO_PKG_VERSION").to_string(),
        };
        write_json_atomic(&path, &doc)
    }

    pub fn db_url(&self) -> String {
        format!(
            "sqlite:{}",
            self.db_path.display().to_string().replace('\\', "/")
        )
    }

    pub fn runtime_backend_json(&self) -> PathBuf {
        self.runtime_dir.join("backend.json")
    }

    pub fn unfiled_matter_dir(&self) -> PathBuf {
        self.matters_dir.join("_unfiled")
    }

    pub fn item_path(&self, matter_slug: &str, kind: &str, item_id: &str) -> PathBuf {
        self.matters_dir
            .join(matter_slug)
            .join("items")
            .join(format!("{kind}-{item_id}.md"))
    }
}

pub fn slugify(input: &str) -> String {
    let mut slug = String::new();
    let mut last_dash = false;
    for ch in input.chars() {
        let lower = ch.to_ascii_lowercase();
        if lower.is_ascii_alphanumeric() {
            slug.push(lower);
            last_dash = false;
        } else if !last_dash && !slug.is_empty() {
            slug.push('-');
            last_dash = true;
        }
        if slug.len() >= 60 {
            break;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() {
        "matter".to_string()
    } else {
        slug
    }
}

pub fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let data = serde_json::to_vec_pretty(value)?;
    write_atomic(path, &data)
}

pub fn write_atomic(path: &Path, data: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!(
        "{}.tmp",
        path.extension()
            .and_then(|v| v.to_str())
            .unwrap_or("mike")
    ));
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}
