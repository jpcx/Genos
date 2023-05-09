use std::{path::PathBuf, sync::Arc};

use tempfile::tempdir;

use crate::test::Test;

/// Genos is an autograder execution environment. It takes care of executing a series of tests in
/// parallel, collating results and writing them to output. It will run each test in it's own
/// temp directory.
#[derive(Default)]
pub struct Genos {
    workspace: PathBuf,
    setup: Vec<Arc<dyn Test>>,
    tests: Vec<Arc<dyn Test>>,
    // todo:
    // add filter
    // add results writers
}

impl Genos {
    pub fn builder() -> GenosBuilder {
        GenosBuilder::default()
    }
}

#[derive(Default)]
pub struct GenosBuilder {
    ws: Option<PathBuf>,
    genos: Genos,
}

impl GenosBuilder {
    /// this is used to designate a workspace for genos to run all of its tests in. Recommended to
    /// use a temporary file and clean it up when genos is finish running if possible. If not
    /// provided, genos will create its own temporary directory and use that as the workspace root.
    pub fn workspace(&mut self, ws: PathBuf) -> &mut Self {
        self.ws = Some(ws);
        self
    }

    /// Setup are special tests which are run before any of the actual tests. Failing these tests
    /// results will cause Genos to skip running any additional unit tests. These are commonly used
    /// to setup the testing environment, validate the submission, etc.
    pub fn setup<T: Test + 'static>(&mut self, setup: T) -> &mut Self {
        self.genos.setup.push(Arc::new(setup));
        self
    }

    //// Use this function to add a new test which will be run during the test execution phase of
    /// genos. Tests are not guarenteed to be executed in the same order they are added.
    pub fn test<T: Test + 'static>(&mut self, test: T) -> &mut Self {
        self.genos.tests.push(Arc::new(test));
        self
    }

    /// Use this to add a collection of tests in a batch. Tests are not guarenteed to be executed
    /// in the same order they are added.
    pub fn tests(&mut self, tests: impl IntoIterator<Item = Arc<dyn Test>>) -> &mut Self {
        self.genos.tests.extend(tests.into_iter());
        self
    }

    /// Build an instance of Genos.
    pub fn build(self) -> Genos {
        let mut genos = self.genos;
        genos.workspace = self.ws.unwrap_or(tempdir().unwrap().into_path());
        genos
    }
}
