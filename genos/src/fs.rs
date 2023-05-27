use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use std::result::Result as StdResult;
use thiserror::Error;

use tokio::{fs::File, io::AsyncReadExt};

#[derive(Debug, Error)]
pub enum Error {
    #[error("File not found")]
    NotFound,

    #[error("TestId not recognized")]
    UnknownTestId,
}

/// Responsible for locating a file resource for a test given a filename,
pub trait ResourceLocator: Send + Sync {
    fn find(&self, name: &String) -> StdResult<PathBuf, Error>;
}

/// can create a resource locator based on the ws
pub trait ResourceLocatorCreator {
    fn create(&self, ws: &Path) -> Box<dyn ResourceLocator>;
}

impl<F> ResourceLocatorCreator for F
where
    F: Fn(&Path) -> Box<dyn ResourceLocator>,
{
    fn create(&self, ws: &Path) -> Box<dyn ResourceLocator> {
        (self)(ws)
    }
}

pub fn filepath<'a>(file: &'a Path) -> Result<&'a str> {
    Ok(file.as_os_str().to_str().ok_or(anyhow!(
        "Could not convert os str to str for {}",
        file.display()
    ))?)
}

pub fn filename<'a>(file: &'a Path) -> Result<&'a str> {
    Ok(file
        .file_name()
        .ok_or(anyhow!("Could not get filename of {}", file.display()))?
        .to_str()
        .ok_or(anyhow!(
            "Could not convert OsStr to str for file {}",
            file.display()
        ))?)
}

pub async fn read_file(path: &Path) -> Result<String> {
    let mut file = File::open(path).await?;
    let mut contents = String::new();
    file.read_to_string(&mut contents).await?;
    Ok(contents)
}
