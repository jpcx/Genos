use std::{
    collections::VecDeque,
    fs::File,
    io::Write,
    path::{Path, PathBuf},
    result::Result as StdResult,
    sync::{Arc, Mutex},
};

use anyhow::{anyhow, Result};
use async_trait::async_trait;

use tempfile::TempDir;

use crate::{
    fs::{self, ResourceLocator},
    process::{self, Command, ExitStatus, ProcessExecutor},
};

pub fn create_temp_file_in<P, N, C>(path: P, name: N, contents: C) -> PathBuf
where
    P: AsRef<Path>,
    N: AsRef<Path>,
    C: AsRef<[u8]>,
{
    let path = path.as_ref().join(name);
    let mut file = File::create(&path).expect(&format!(
        "Expected dir {} to exist",
        path.parent().unwrap().display()
    ));
    file.write_all(contents.as_ref()).unwrap();

    path
}

pub struct MockDir {
    pub root: TempDir,
}

impl MockDir {
    pub fn new() -> Self {
        Self {
            root: tempfile::tempdir().unwrap(),
        }
    }

    pub fn file<T: Into<MockFile>>(self, file: T) -> Self {
        let file = file.into();
        create_temp_file_in(self.root.path(), &file.name, &file.contents);
        self
    }

    pub fn dir<T: Into<MockDir>, N: AsRef<std::path::Path>>(self, name: N, dir: T) -> Self {
        let new_dir = dir.into();
        let new_dir = new_dir.root.into_path();
        let dest = self.root.path().join(name);
        std::fs::rename(new_dir, dest).unwrap();
        self
    }

    pub fn path_from_root<P: AsRef<Path>>(&self, relative_path: P) -> PathBuf {
        self.root.path().join(relative_path)
    }
}

// This should really search all dirctories and files recursively. Right now it only searches the
// top level
impl ResourceLocator for MockDir {
    fn find(&self, name: &String) -> StdResult<PathBuf, fs::Error> {
        let file = self.root.path().join(name);
        if !file.exists() {
            return Err(fs::Error::NotFound);
        }

        Ok(file)
    }
}

pub struct MockFile {
    pub name: String,
    pub contents: Vec<u8>,
}

impl MockFile {
    pub fn new<N, C>(name: N, contents: C) -> Self
    where
        N: Into<String>,
        C: Into<Vec<u8>>,
    {
        Self {
            name: name.into(),
            contents: contents.into(),
        }
    }
}

impl<A, B> From<(A, B)> for MockFile
where
    A: Into<String>,
    B: Into<Vec<u8>>,
{
    fn from(value: (A, B)) -> Self {
        MockFile::new(value.0, value.1)
    }
}

pub struct MockExecutorInner {
    pub commands: Vec<Command>,
    pub responses: VecDeque<Result<process::Output>>,
    pub default: Result<process::Output>,
}

impl MockExecutorInner {
    pub fn with_responses<I: IntoIterator<Item = Result<process::Output>>>(resp: I) -> Self {
        Self {
            commands: Vec::new(),
            responses: resp.into_iter().collect(),
            default: Ok(process::Output {
                status: ExitStatus::Ok,
                stdout: "".to_string(),
                stderr: "".to_string(),
            }),
        }
    }
}

#[derive(Clone)]
pub struct MockProcessExecutor {
    pub inner: Arc<Mutex<MockExecutorInner>>,
}

impl MockProcessExecutor {
    pub fn new(inner: Arc<Mutex<MockExecutorInner>>) -> Self {
        Self { inner }
    }

    pub fn with_responses<I: IntoIterator<Item = Result<process::Output>>>(resp: I) -> Self {
        Self {
            inner: Arc::new(Mutex::new(MockExecutorInner::with_responses(resp))),
        }
    }
}

#[async_trait]
impl ProcessExecutor for MockProcessExecutor {
    async fn run(&self, cmd: &Command) -> Result<process::Output> {
        let mut inner = self.inner.lock().unwrap();
        inner.commands.push(cmd.clone());

        match inner.responses.pop_front() {
            Some(resp) => resp,
            None => match &inner.default {
                Ok(res) => Ok(res.clone()),
                Err(e) => Err(anyhow!("Mock error: {e}")),
            },
        }
    }
}
