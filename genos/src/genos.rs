use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::{
    gs::Description,
    output::Output,
    test::{Test, TestResult},
    writer::{ResultsWriter, TestOutput},
};
use anyhow::{anyhow, Context, Error, Result};
use futures::future::join_all;
use tempfile::tempdir;
use tokio::fs::create_dir;
use tracing::{error, instrument};

pub trait TestRequest: Test + Description {}
impl<T> TestRequest for T where T: Test + Description {}

/// Genos is an autograder execution environment. It takes care of executing a series of tests in
/// parallel, collating results and writing them to output. It will run each test in it's own
/// temp directory.
#[derive(Default)]
pub struct Genos {
    workspace: PathBuf,
    setup: Vec<Arc<dyn TestRequest>>,
    tests: Vec<Arc<dyn TestRequest>>,
    writers: Vec<Arc<dyn ResultsWriter>>,
    // add a way to prepare a workspace
    //  - This will be the mechanism which will copy over files from staging directory into the
    //    workspace directory
    //
    // add filter
    //  - filter will control which tests are run.
    //      - takes into account cli args, groupings, etc
}

impl Genos {
    pub fn builder() -> GenosBuilder {
        GenosBuilder::default()
    }

    pub async fn run(&self) -> Result<Vec<TestResult>> {
        let res = self.run_all_tests().await;
        self.write_results(&res).await;

        // transform the run result into something ingestible by consumers. If there was a system
        // error then return the first found, otherwise return the vector of test results.
        res.into_iter().fold(Ok(vec![]), |acc, curr| match acc {
            Ok(mut results) => match &curr.err {
                Some(e) => Err(anyhow!("Found system error: {}", e)),
                None => {
                    results.push(curr.res.clone());
                    return Ok(results);
                }
            },
            Err(e) => Err(e), // should probably find a way to collate ALL the system errors we
                              // find instead of just bubbling up the first one.
        })
    }

    async fn run_all_tests(&self) -> Vec<Arc<RunResult>> {
        let mut results = Vec::new();

        // first, run the setup test cases serially
        for setup_test in &self.setup {
            let res = run_test_and_process_result(self.workspace.clone(), setup_test.clone()).await;
            let is_err = res.err.is_some();

            results.push(Arc::new(res));

            if is_err {
                return results;
            }
        }

        // run all the other tests in parallel
        let test_results = join_all(self.tests.iter().map(|test| {
            let test = test.clone();
            let ws = self.workspace.clone();
            async move {
                let res = run_test_and_process_result(ws, test.clone()).await;
                Arc::new(res)
            }
        }))
        .await;

        results.extend(test_results);
        results
    }

    async fn write_results(&self, res: &Vec<Arc<RunResult>>) {
        join_all(self.writers.iter().map(|writer| {
            let writer = writer.clone();
            let res = res.iter().map(|res| res.as_test_output()).collect();

            async move { writer.write(res).await }
        }))
        .await
        .into_iter()
        .for_each(|res| res.unwrap());
    }
}

struct RunResult {
    test: Arc<dyn TestRequest>,
    res: TestResult,
    err: Option<Error>,
}

impl RunResult {
    fn as_test_output(self: &Arc<Self>) -> Arc<dyn TestOutput> {
        self.clone()
    }
}

impl Description for RunResult {
    fn name(&self) -> String {
        self.test.name()
    }

    fn description(&self) -> String {
        self.test.description()
    }

    fn visibility(&self) -> crate::gs::Visibility {
        self.test.visibility()
    }

    fn id(&self) -> crate::tid::TestId {
        Description::id(self.test.as_ref())
    }
}

impl TestOutput for RunResult {
    fn status(&self) -> crate::test::TestStatus {
        self.res.status.clone()
    }

    fn output(&self) -> Output {
        self.res.output.clone()
    }
}

fn test_workspace_id(test: &Arc<dyn TestRequest>) -> String {
    format!("test_{}", test.id())
}

#[instrument(skip_all, fields(ws = ?ws.display(), id = ?test.id()))]
async fn run_test_and_process_result(ws: PathBuf, test: Arc<dyn TestRequest>) -> RunResult {
    match run_test(ws.as_path(), &test).await {
        Ok(res) => RunResult {
            test,
            res,
            err: None,
        },
        Err(err) => {
            error!("system error: {err}");

            let mut res = TestResult::new(test.points());
            res.lose_full_points();
            res.output.append(Output::new().section((
                "System Error Occurred",
                "Please report this to course staff",
            )));

            RunResult {
                test,
                res,
                err: Some(err),
            }
        }
    }
}

async fn run_test(ws: &Path, test: &Arc<dyn TestRequest>) -> Result<TestResult> {
    let test_ws = ws.join(test_workspace_id(&test));
    create_dir(&test_ws)
        .await
        .context(format!("Error creating directory for test {}", test.id()))?;

    test.run(&test_ws).await
}

#[derive(Default)]
pub struct GenosBuilder {
    ws: Option<PathBuf>,
    genos: Genos,
    writers: Vec<Arc<dyn ResultsWriter>>,
}

impl GenosBuilder {
    /// this is used to designate a workspace for genos to run all of its tests in. Recommended to
    /// use a temporary file and clean it up when genos is finish running if possible. If not
    /// provided, genos will create its own temporary directory and use that as the workspace root.
    pub fn workspace(mut self, ws: PathBuf) -> Self {
        self.ws = Some(ws);
        self
    }

    /// Setup are special tests which are run before any of the actual tests. Failing these tests
    /// results will cause Genos to skip running any additional unit tests. These are commonly used
    /// to setup the testing environment, validate the submission, etc.
    pub fn setup<T: TestRequest + 'static>(mut self, setup: T) -> Self {
        self.genos.setup.push(Arc::new(setup));
        self
    }

    pub fn setups<I, T>(mut self, tests: I) -> Self
    where
        I: IntoIterator<Item = T>,
        T: TestRequest + 'static,
    {
        self.genos
            .setup
            .extend(tests.into_iter().map(|req| Arc::new(req) as _));
        self
    }

    pub fn add_setup<T: TestRequest + 'static>(&mut self, setup: T) {
        self.genos.setup.push(Arc::new(setup));
    }

    //// Use this function to add a new test which will be run during the test execution phase of
    /// genos. Tests are not guarenteed to be executed in the same order they are added.
    pub fn test<T: TestRequest + 'static>(mut self, test: T) -> Self {
        self.genos.tests.push(Arc::new(test));
        self
    }

    pub fn add_test<T: TestRequest + 'static>(&mut self, test: T) {
        self.genos.tests.push(Arc::new(test));
    }

    /// Use this to add a collection of tests in a batch. Tests are not guarenteed to be executed
    /// in the same order they are added.
    pub fn tests<I, T>(mut self, tests: I) -> Self
    where
        I: IntoIterator<Item = T>,
        T: TestRequest + 'static,
    {
        self.genos
            .tests
            .extend(tests.into_iter().map(|req| Arc::new(req) as _));
        self
    }

    /// Use this to tell genos how to handle the results. Writers can be used to tell genos how to
    /// handle the results, typically by writing to a results.json file in gs.
    pub fn writer(mut self, writer: impl ResultsWriter + 'static) -> Self {
        self.writers.push(Arc::new(writer));
        self
    }

    /// Build an instance of Genos.
    pub fn build(self) -> Genos {
        let mut genos = self.genos;
        // purposefully move the tempdir into an owned path. This removes the property that the
        // tempdir will get deleted when it is dropped. This is so graders can observe the state of
        // the system after it is finished running.
        genos.workspace = self.ws.unwrap_or(tempdir().unwrap().into_path());
        genos.writers = self.writers;
        genos
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        atomic::{AtomicU32, Ordering},
        Mutex,
    };

    use async_trait::async_trait;

    use crate::{
        output::Contains, points::Points, score::Score, test::TestStatus, tid::TestId, Executor,
    };

    use super::*;

    struct MockTest {
        res: Arc<Mutex<Option<Result<TestResult>>>>,
        id: TestId,
    }

    impl MockTest {
        fn new(id: TestId, res: Result<TestResult>) -> Self {
            Self {
                res: Arc::new(Mutex::new(Some(res))),
                id,
            }
        }

        fn into_dyn_request(self: Arc<Self>) -> Arc<dyn TestRequest> {
            self
        }
    }

    impl Description for MockTest {
        fn name(&self) -> String {
            "".to_string()
        }

        fn description(&self) -> String {
            "".to_string()
        }

        fn visibility(&self) -> crate::gs::Visibility {
            crate::gs::Visibility::Hidden
        }

        fn id(&self) -> crate::tid::TestId {
            self.id
        }
    }

    impl Test for MockTest {
        fn points(&self) -> crate::points::Points {
            Points::new(1)
        }
    }

    #[async_trait]
    impl Executor for MockTest {
        type Output = TestResult;
        async fn run(&self, _ws: &Path) -> Result<Self::Output> {
            self.res.lock().unwrap().take().unwrap()
        }
    }

    fn get_tests_with_results(
        results: impl IntoIterator<Item = Result<TestResult>>,
    ) -> Vec<MockTest> {
        static ID: AtomicU32 = AtomicU32::new(0);

        results
            .into_iter()
            .map(|res| {
                let id = ID.fetch_add(1, Ordering::Relaxed);
                let test = MockTest::new(TestId::new(id), res);
                println!("Made test with id {}", test.id);
                test
            })
            .collect()
    }

    #[tokio::test]
    async fn run_process_success() {
        let test: Arc<dyn TestRequest> = Arc::new(MockTest::new(
            TestId::new(0),
            Ok(TestResult::new(Points::new(1))),
        ));

        let ws = tempfile::tempdir().unwrap().into_path();
        let run_result = run_test_and_process_result(ws, test).await;
        assert!(run_result.err.is_none(), "{:?}", run_result.err);
    }

    #[tokio::test]
    async fn run_process_system_error() {
        let test: Arc<dyn TestRequest> =
            Arc::new(MockTest::new(TestId::new(0), Err(anyhow!("System error"))));

        let ws = tempfile::tempdir().unwrap().into_path();
        let run_result = run_test_and_process_result(ws, test).await;
        assert!(run_result.err.is_some());
        assert!(run_result.res.output.contains("System Error Occurred"));
        assert_eq!(
            run_result.res.status,
            TestStatus::Fail(Score::zero_points(Points::new(1)))
        );
    }

    #[tokio::test]
    async fn creates_test_ws() {
        let test: Arc<dyn TestRequest> = Arc::new(MockTest::new(
            TestId::new(0),
            Ok(TestResult::new(Points::new(1))),
        ));

        let tempdir = tempfile::tempdir().unwrap().into_path();
        let run_result = run_test_and_process_result(tempdir.clone(), test.clone()).await;
        assert!(run_result.err.is_none());
        let test_ws = tempdir.join(test_workspace_id(&test));
        assert!(test_ws.exists());
    }

    struct MockWriter {
        results: Arc<Mutex<Option<Vec<Arc<dyn TestOutput>>>>>,
    }

    #[async_trait]
    impl ResultsWriter for MockWriter {
        async fn write(&self, results: Vec<Arc<dyn TestOutput>>) -> Result<()> {
            *self.results.lock().unwrap() = Some(results);
            Ok(())
        }
    }

    #[tokio::test]
    async fn stops_at_setup_error() {
        let setup_tests = get_tests_with_results([
            Ok(TestResult::new(Points::new(1))),
            Err(anyhow!("System error")),
            Ok(TestResult::new(Points::new(1))),
        ]);
        let real_tests = get_tests_with_results([
            Ok(TestResult::new(Points::new(1))),
            Ok(TestResult::new(Points::new(1))),
        ]);

        let results = Arc::new(Mutex::new(None));
        let writer = MockWriter {
            results: results.clone(),
        };

        let genos = Genos::builder()
            .setups(setup_tests)
            .tests(real_tests)
            .writer(writer)
            .build();

        let _ = genos.run().await.unwrap_err();

        let results = results.lock().unwrap().take().unwrap();
        assert_eq!(results.len(), 2);
    }

    #[tokio::test]
    async fn multiple_system_errors_in_tests() {
        let real_tests = get_tests_with_results([
            Ok(TestResult::new(Points::new(1))),
            Err(anyhow!("System error")),
            Ok(TestResult::new(Points::new(1))),
            Err(anyhow!("System error")),
            Ok(TestResult::new(Points::new(1))),
            Err(anyhow!("System error")),
            Ok(TestResult::new(Points::new(1))),
        ]);

        let results = Arc::new(Mutex::new(None));
        let writer = MockWriter {
            results: results.clone(),
        };

        let genos = Genos::builder().tests(real_tests).writer(writer).build();

        let _ = genos.run().await.unwrap_err();

        let results = results.lock().unwrap().take().unwrap();
        assert_eq!(results.len(), 7);
    }

    #[tokio::test]
    async fn all_successful() {
        let setup_tests = get_tests_with_results([
            Ok(TestResult::new(Points::new(1))),
            Ok(TestResult::new(Points::new(1))),
        ]);
        let real_tests = get_tests_with_results([
            Ok(TestResult::new(Points::new(1))),
            Ok(TestResult::new(Points::new(1))),
            Ok(TestResult::new(Points::new(1))),
            Ok(TestResult::new(Points::new(1))),
        ]);

        let results = Arc::new(Mutex::new(None));
        let writer = MockWriter {
            results: results.clone(),
        };

        let genos = Genos::builder()
            .setups(setup_tests)
            .tests(real_tests)
            .writer(writer)
            .build();

        let _ = genos.run().await.unwrap();

        let results = results.lock().unwrap().take().unwrap();
        assert_eq!(results.len(), 6);
    }
}
