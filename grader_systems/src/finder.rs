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

use std::path::PathBuf;

use anyhow::{anyhow, Result};
use genos::{fs::ResourceLocator, tid::TestId};

fn tid_to_dir_name(tid: TestId) -> String {
    format!("test_{}", tid)
}

pub trait TestResourceFinder {
    fn test_resource(&self, tid: TestId, name: &String) -> Result<PathBuf>;
}

pub trait SystemResourceFinder {
    fn system_resource(&self, name: &String) -> Result<PathBuf>;
}

pub struct DirFinder {
    dir: PathBuf,
}

impl DirFinder {
    pub fn new(dir: PathBuf) -> Self {
        assert!(dir.is_dir());
        Self { dir }
    }
}

impl ResourceLocator for DirFinder {
    fn find(&self, name: &String) -> Result<PathBuf> {
        let file = self.dir.join(name);
        if !file.exists() {
            return Err(anyhow!("File not found: {:?}", file.display()));
        }

        Ok(file)
    }
}

#[derive(Clone)]
pub struct FileFinder {
    root: PathBuf,
    class: PathBuf,
    hw: PathBuf,
    system: PathBuf,
}

impl TestResourceFinder for FileFinder {
    fn test_resource(&self, tid: TestId, name: &String) -> Result<PathBuf> {
        let test_dir = self.hw.join(tid_to_dir_name(tid));
        if !test_dir.exists() {
            return Err(anyhow!("File not found: {:?}", test_dir.display()));
        }

        Ok(test_dir)
    }
}

impl SystemResourceFinder for FileFinder {
    fn system_resource(&self, name: &String) -> Result<PathBuf> {
        let resource = self.system.join(name);
        if !resource.exists() {
            return Err(anyhow!("File not found: {:?}", resource.display()));
        }

        Ok(resource)
    }
}

struct TestFileFinder<F> {
    tid: TestId,
    finder: F,
}

impl<F> ResourceLocator for TestFileFinder<F>
where
    F: TestResourceFinder + Send + Sync,
{
    fn find(&self, name: &String) -> Result<PathBuf> {
        self.finder.test_resource(self.tid, name)
    }
}
