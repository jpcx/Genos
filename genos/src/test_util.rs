use std::{
    collections::{HashMap, VecDeque},
    fs::File,
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use tempfile::TempDir;

use crate::{
    fs::ResourceLocator,
    process::{self, Command, ExitStatus, ProcessExecutor},
};

pub fn create_temp_file_in<P, N, C>(path: P, name: N, contents: C) -> PathBuf
where
    P: AsRef<Path>,
    N: AsRef<Path>,
    C: AsRef<[u8]>,
{
    let path = path.as_ref().join(name);
    let mut file = File::create(&path).unwrap();
    file.write_all(contents.as_ref()).unwrap();

    path
}

pub struct MockDir {
    pub root: TempDir,
    pub files: HashMap<String, PathBuf>,
}

impl MockDir {
    pub fn new() -> Self {
        Self {
            root: tempfile::tempdir().unwrap(),
            files: HashMap::default(),
        }
    }

    pub fn file<T: Into<MockFile>>(mut self, file: T) -> Self {
        let file = file.into();
        let path = create_temp_file_in(self.root.path(), &file.name, &file.contents);
        self.files.insert(file.name, path);
        self
    }
}

impl ResourceLocator for MockDir {
    fn find(&self, name: &String) -> Result<PathBuf> {
        self.files
            .get(name)
            .cloned()
            .ok_or(anyhow!("Could not find test file"))
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
