use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use serde::Deserialize;
use tokio::fs::copy;
use tracing::debug;

use crate::fs::ResourceLocator;

use super::SystemStageExecutor;

#[derive(Default)]
pub struct ImportFiles {
    files: Vec<PathBuf>,
}

impl ImportFiles {
    pub fn new<F: ResourceLocator>(config: &ImportConfig, finder: &F) -> Result<Self> {
        let mut imports = ImportFiles::default();
        for file_name in &config.files {
            imports.files.push(finder.find(&file_name)?);
        }

        Ok(imports)
    }
}

#[async_trait]
impl SystemStageExecutor for ImportFiles {
    async fn run(&self, ws: &Path) -> Result<()> {
        for file in &self.files {
            let to = ws.join(
                file.file_name()
                    .ok_or(anyhow!("could not get filename for {}", file.display()))?,
            );

            debug!(src=?file, dest=?to, "copying file");
            copy(file, to).await?;
        }
        Ok(())
    }
}

#[derive(Debug, Default, Deserialize, Clone)]
pub struct ImportConfig {
    files: Vec<String>,
}

impl ImportConfig {
    pub fn new(files: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            files: files.into_iter().map(|f| f.into()).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use tokio::{fs::File, io::AsyncReadExt};

    use crate::test_util::{create_temp_file_in, MockDir};

    use super::*;

    #[tokio::test]
    async fn copies_files() {
        let dir1 = tempfile::tempdir().unwrap();
        let dir2 = tempfile::tempdir().unwrap();
        let ws = tempfile::tempdir().unwrap();

        let f1 = create_temp_file_in(&dir1, "file1.txt", "file1");
        let f2 = create_temp_file_in(&dir2, "file2.txt", "file2");

        assert!(f1.try_exists().unwrap());
        assert!(f2.try_exists().unwrap());

        let import = ImportFiles {
            files: vec![f1.clone(), f2.clone()],
        };

        import.run(ws.path()).await.unwrap();

        assert!(ws.path().join(f1.file_name().unwrap()).exists());
        assert!(ws.path().join(f2.file_name().unwrap()).exists());

        let mut f1 = File::open(f1).await.unwrap();
        let mut f2 = File::open(f2).await.unwrap();

        let mut contents = Vec::new();
        f1.read_to_end(&mut contents).await.unwrap();
        assert_eq!(&contents, "file1".as_bytes());

        contents.clear();
        f2.read_to_end(&mut contents).await.unwrap();
        assert_eq!(&contents, "file2".as_bytes());
    }

    #[tokio::test]
    async fn import_config_to_executor() {
        let config = ImportConfig::new(["file1", "file2"]);

        let data = MockDir::new()
            .file(("file1", "file1 contents"))
            .file(("file2", "file2 contents"));

        ImportFiles::new(&config, &data).unwrap();
    }
}
