/*
 * Expects the data folder to look like
 * data/
 *     system/
 *         // for files which are used by the autograder
 *     2022-fall/
 *         lib/
 *         // all tests for each hw for this quarter
 *         hw1/
 *         hw2/
 *             hw.yaml // config file for the hw
 *
 *             // test files associated with hw2
 *             gs/
 *                 // files used by gradescope
 *                 run_autograder
 *                 setup.sh
 *             static/
 *                 // files used for multiple test cases
 *             basefiles/
 *                 // files copied to each test case
 *             test_1/
 *                 // files for test 1
 *                 test.yaml // config file for this test
 *             test_2/
 */

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use futures::future::join_all;
use genos::{
    fs::{filename, filepath, Error, ResourceLocator},
    tid::TestId,
};

use glob::glob;
use tokio::{fs::File, io::AsyncReadExt};
use tracing::{debug, warn};

use crate::config::{FromConfigFile, TestConfig, TEST_CONFIG_NAME};

pub trait TestResourceFinder {
    fn test_resource(&self, tid: TestId, name: &String) -> Result<PathBuf, Error>;
}

pub trait SystemResourceFinder {
    fn system_resource(&self, name: &String) -> Result<PathBuf, Error>;
}

#[async_trait]
pub trait TestConfigFinder {
    async fn load_test_configs(&self) -> Result<Vec<TestConfig>>;
}

pub struct DirFinder {
    dir: PathBuf,
}

impl DirFinder {
    pub fn new(dir: PathBuf) -> Self {
        assert!(dir.is_dir(), "expected dir, but found {:?}", dir.display());
        Self { dir }
    }
}

impl ResourceLocator for DirFinder {
    fn find(&self, name: &String) -> Result<PathBuf, Error> {
        let file = self.dir.join(name);
        if file.is_dir() || !file.exists() {
            return Err(Error::NotFound);
        }

        Ok(file)
    }
}

pub struct Finder {
    // directories for each test. test_resource_dirs is the first place searched for when
    // attempting to retrieve resources for a test and are associated with the `test_X` directory
    // found in the hw directory.
    test_resource_dirs: HashMap<TestId, Box<dyn ResourceLocator>>,

    // static is a directory found in the hw root and is a place for files used by multiple tests
    // in that hw. It is searched if the requested resource does not exist in the hw directory.
    static_resource_dir: Option<Box<dyn ResourceLocator>>,

    // system resource dir is the directory holding system files which are required for the
    // autograder to function, such as the unittest header.
    system_resource_dir: Box<dyn ResourceLocator>,
}

impl Finder {
    pub fn new(
        test_resource_dirs: HashMap<TestId, Box<dyn ResourceLocator>>,
        system_resource_dir: Box<dyn ResourceLocator>,
        static_resource_dir: Option<Box<dyn ResourceLocator>>,
    ) -> Self {
        Self {
            test_resource_dirs,
            static_resource_dir,
            system_resource_dir,
        }
    }

    // Expect the data directory to have a structure which can be read based on the hw config and
    // its location. For example, by knowing where the hw config is, we know the system dir is 2
    // levels up, and that the test resource dirs are in the same direcory and follow the naming
    // convention test_X.
    pub fn from_hw_config_path(hw_config: &Path) -> Result<Self> {
        // make the path absolute
        let hw_config = std::fs::canonicalize(hw_config)?;
        assert!(hw_config.is_file(), "Expected hw config to be a file");

        let hw_root = hw_config
            .parent()
            .context("Expected hw config to have a parent")?;
        assert!(hw_root.is_dir());

        // walk all the test_X directories, constructing dirfinders as we go.
        let test_resource_dirs = glob(format!("{}/test_*", filepath(hw_root)?).as_str())?
            .filter_map(|entry| match entry {
                Ok(test_dir) => {
                    debug!("found test dir {:?}", test_dir.display());
                    let filename = filename(&test_dir).unwrap();
                    let (_, id) = filename.split_once('_').unwrap();
                    let id = match id.parse() {
                        Ok(id) => id,
                        Err(_) => return None,
                    };
                    let test_id = TestId::new(id);
                    let finder: Box<dyn ResourceLocator> = Box::new(DirFinder::new(test_dir));
                    Some((test_id, finder))
                }
                Err(e) => {
                    warn!("Could not read test directory, skipping: {:?}", e);
                    None
                }
            })
            .collect();

        // the static directory is optional and is found in the hw root
        let dir = hw_root.join("static");
        let static_resource_dir: Option<Box<dyn ResourceLocator>> = dir
            .try_exists()?
            // need the cast here to coerce the DirFinder into a trait object
            .then(|| Box::new(DirFinder::new(dir)) as _);

        // the data root is two levels up from the hw root.
        let data_root = hw_root
            .parent()
            .ok_or(anyhow!("hw root has no parent"))?
            .parent()
            .ok_or(anyhow!("could not locate data root"))?;

        let system_resource_dir = data_root.join("system");

        if !system_resource_dir.exists() {
            return Err(anyhow!("expected system resource dir at data root"));
        }

        Ok(Self::new(
            test_resource_dirs,
            Box::new(DirFinder::new(system_resource_dir)),
            static_resource_dir,
        ))
    }
}

impl TestResourceFinder for Finder {
    fn test_resource(&self, tid: TestId, name: &String) -> Result<PathBuf, Error> {
        let resource_dir = self
            .test_resource_dirs
            .get(&tid)
            .ok_or(Error::UnknownTestId)?;

        resource_dir
            .find(name)
            .or_else(|_e| match self.static_resource_dir.as_ref() {
                Some(dir) => dir.find(name),
                None => Err(Error::NotFound),
            })
    }
}

impl SystemResourceFinder for Finder {
    fn system_resource(&self, name: &String) -> Result<PathBuf, Error> {
        self.system_resource_dir.find(name)
    }
}

#[async_trait]
impl TestConfigFinder for Finder {
    async fn load_test_configs(&self) -> Result<Vec<TestConfig>> {
        // load all in configs in parallel
        join_all(self.test_resource_dirs.iter().map(|(tid, dir)| async {
                let path = dir.find(&TEST_CONFIG_NAME.to_string())?;

                let config = TestConfig::from_file(&path).await?;

                assert_eq!(
                    config.description.test_id, *tid,
                    "expected test config test_id to match directory it's contained in. Found {}, expected {}",
                    config.description.test_id, *tid
                );

                Ok(config)
            }))
            .await
            .into_iter()
            .collect()
    }
}

/// TestFileFinder provides a wrapper which can be given to tests which contains context on which
/// test it can provide files for.
pub struct TestFileFinder<F> {
    tid: TestId,
    finder: Arc<F>,
}

impl<F> Clone for TestFileFinder<F> {
    fn clone(&self) -> Self {
        Self {
            tid: self.tid.clone(),
            finder: self.finder.clone(),
        }
    }
}

impl<F> TestFileFinder<F> {
    pub fn new(tid: TestId, finder: Arc<F>) -> Self {
        Self { tid, finder }
    }
}

impl<F> ResourceLocator for TestFileFinder<F>
where
    F: TestResourceFinder + Send + Sync,
{
    fn find(&self, name: &String) -> Result<PathBuf, Error> {
        self.finder.test_resource(self.tid, name)
    }
}

#[derive(Default)]
pub struct MultiSourceFinder {
    finders: Vec<Box<dyn ResourceLocator>>,
}

impl MultiSourceFinder {
    pub fn source(mut self, source: Box<dyn ResourceLocator>) -> Self {
        self.finders.push(source);
        self
    }
}

#[cfg(test)]
impl Finder {
    pub fn from_mock_dir<P: AsRef<std::path::Path>>(
        mock_dir: &genos::test_util::MockDir,
        relative_path_to_hw_config: P,
    ) -> Result<Self> {
        let hw_config = mock_dir.path_from_root(relative_path_to_hw_config);
        if !hw_config.exists() {
            genos::test_util::create_temp_file_in(
                &hw_config.parent().unwrap(),
                "hw.yaml",
                "hw config contents",
            );
        }

        Self::from_hw_config_path(&hw_config)
    }
}

#[cfg(test)]
mod tests {
    use genos::test_util::{MockDir, MockFile};

    use super::*;

    #[test]
    fn new_finder_from_existing_directory() {
        let mock_data_dir = MockDir::new()
            .dir(
                "system",
                MockDir::new().file(MockFile::new("genos_unittest.h", "contents")),
            )
            .dir(
                "2022-winter",
                MockDir::new().dir(
                    "hw1",
                    MockDir::new()
                        .file(MockFile::new("hw.yaml", "hw config contents"))
                        .dir("test_1", MockDir::new())
                        .dir("test_2", MockDir::new())
                        .dir("test_3", MockDir::new()),
                ),
            );

        let hw_config = mock_data_dir.path_from_root("2022-winter/hw1/hw.yaml");
        assert!(hw_config.exists());
        assert!(hw_config.is_file());

        let finder = Finder::from_hw_config_path(&hw_config).unwrap();
        for tid in [1, 2, 3] {
            let tid = TestId::new(tid);
            assert!(finder.test_resource_dirs.contains_key(&tid));
        }

        assert!(finder.static_resource_dir.is_none());
    }

    #[test]
    fn new_finder_from_mock_dir() {
        let mock_data_dir = MockDir::new()
            .dir(
                "system",
                MockDir::new().file(MockFile::new("genos_unittest.h", "contents")),
            )
            .dir(
                "2022-winter",
                MockDir::new().dir(
                    "hw1",
                    MockDir::new()
                        // don't need to specify hw config
                        .dir("test_1", MockDir::new())
                        .dir("test_2", MockDir::new())
                        .dir("test_3", MockDir::new()),
                ),
            );

        Finder::from_mock_dir(&mock_data_dir, "2022-winter/hw1/hw.yaml").unwrap();
    }

    #[test]
    fn test_file_finder() {
        let mock_data_dir = MockDir::new()
            .dir(
                "system",
                MockDir::new().file(MockFile::new("genos_unittest.h", "contents")),
            )
            .dir(
                "2022-winter",
                MockDir::new().dir(
                    "hw1",
                    MockDir::new()
                        // don't need to specify hw config
                        .dir(
                            "test_1",
                            MockDir::new()
                                .file(MockFile::new("expected_stdout", "expected stdout content"))
                                .file(MockFile::new("expected_stderr", "expected stderr content")),
                        )
                        .dir("test_2", MockDir::new())
                        .dir("test_3", MockDir::new()),
                ),
            );

        let finder =
            Arc::new(Finder::from_mock_dir(&mock_data_dir, "2022-winter/hw1/hw.yaml").unwrap());
        let test_finder = TestFileFinder::new(1.into(), finder.clone());

        let path = test_finder.find(&"expected_stderr".to_string()).unwrap();
        assert!(path.exists());
        assert_eq!(filename(&path).unwrap(), "expected_stderr");
    }

    #[test]
    fn test_file_finder_cant_locate_file() {
        let mock_data_dir = MockDir::new()
            .dir(
                "system",
                MockDir::new().file(MockFile::new("genos_unittest.h", "contents")),
            )
            .dir(
                "2022-winter",
                MockDir::new().dir(
                    "hw1",
                    MockDir::new()
                        // don't need to specify hw config
                        .dir(
                            "test_1",
                            MockDir::new()
                                .file(MockFile::new("expected_stdout", "expected stdout content"))
                                .file(MockFile::new("expected_stderr", "expected stderr content")),
                        )
                        .dir("test_2", MockDir::new())
                        .dir("test_3", MockDir::new()),
                ),
            );

        let finder =
            Arc::new(Finder::from_mock_dir(&mock_data_dir, "2022-winter/hw1/hw.yaml").unwrap());
        let test_finder = TestFileFinder::new(2.into(), finder.clone());
        test_finder
            .find(&"expected_stderr".to_string())
            .unwrap_err();
    }

    #[test]
    fn test_file_finder_searches_static_if_exists() {
        let mock_data_dir = MockDir::new()
            .dir(
                "system",
                MockDir::new().file(MockFile::new("genos_unittest.h", "contents")),
            )
            .dir(
                "2022-winter",
                MockDir::new().dir(
                    "hw1",
                    MockDir::new()
                        // don't need to specify hw config
                        .dir(
                            "test_1",
                            MockDir::new()
                                .file(MockFile::new("expected_stdout", "expected stdout content"))
                                .file(MockFile::new("expected_stderr", "expected stderr content")),
                        )
                        .dir("test_2", MockDir::new())
                        .dir("test_3", MockDir::new())
                        .dir(
                            "static",
                            MockDir::new()
                                .file(MockFile::new("static_file", "static file contents")),
                        ),
                ),
            );

        let finder =
            Arc::new(Finder::from_mock_dir(&mock_data_dir, "2022-winter/hw1/hw.yaml").unwrap());
        let test_finder = TestFileFinder::new(2.into(), finder.clone());
        let path = test_finder.find(&"static_file".to_string()).unwrap();
        assert!(path.exists());
    }
}
