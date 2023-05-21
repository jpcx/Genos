use std::{path::Path, sync::Arc};

use crate::{
    config::{Cli, HwConfig, TestConfig, TestType},
    finder::{Finder, TestConfigFinder, TestFileFinder},
    stage::{compile::Compile, run::Run},
};

use anyhow::{anyhow, Result};
use genos::{
    fs::ResourceLocator,
    process::ShellExecutor,
    stage::{
        compare_files::{ComparatorCreatorImpl, CompareFiles},
        import_files::ImportFiles,
    },
    test::GenosTest,
};

/// Holds all the context required to execute a run of the autograder
pub struct Context {
    cli_config: Arc<Cli>,
    hw_config: Arc<HwConfig>,
    finder: Arc<Finder>,
}

impl Context {
    pub async fn new(cli_config: Cli, hw_config: HwConfig) -> Self {
        let finder = Finder::from_hw_config_path(&cli_config.config).unwrap();
        Self {
            cli_config: Arc::new(cli_config),
            hw_config: Arc::new(hw_config),
            finder: Arc::new(finder),
        }
    }

    pub async fn run_grader(&self) -> Result<()> {
        let _test_configs = self.finder.load_test_configs().await?;

        Ok(())
    }

    fn create_test(&self, config: &TestConfig) -> Result<GenosTest> {
        match &config.test_type {
            TestType::Diff => self.make_diff_test(config),
        }
    }

    // Diff test is used to compare the output produced by the submission to expected output found
    // in test resuorces. It has the following stage order
    // 1. import files (if required)
    // 2. compile assignment
    // 3. run assignment
    // 4. compare output with expected
    // 5. run assignment using valgrind to detect memmory leaks (if configured)
    // 6. run assignment with memory limit to detect excess memory usage (if configured)
    fn make_diff_test(&self, config: &TestConfig) -> Result<GenosTest> {
        let mut test = GenosTest::new(config.description.total_points);
        let test_file_finder = TestFileFinder::new(config.description.test_id, self.finder.clone());

        if let Some(import_files) = &config.import_files {
            test.add_stage(ImportFiles::new(import_files, &test_file_finder)?)
        }

        test.add_stage(Compile::new(&config.compile, ShellExecutor));

        test.add_stage(Run::new(ShellExecutor, config.run.clone()));

        {
            let compare_files = config
                .compare_files
                .as_ref()
                .ok_or(anyhow!("Expected diff test to have at least one compare"))?;

            // need to have a factory here since one of the test types is just like the diff test
            // but also requires searching in the test's current workspace instead of just in the
            // test resource directory for expected output.
            // The current ws is defined by genos at runtime, so we can't know it when creating the
            // test so instead we give a factory that takes in the ws and produces the appropriate
            // finder for that test case.
            let test_file_finder = test_file_finder.clone();
            let locator_creator = move |_path: &Path| {
                let finder: Box<dyn ResourceLocator> = Box::new(test_file_finder.clone());
                finder
            };

            test.add_stage(CompareFiles::new(
                locator_creator,
                ComparatorCreatorImpl::new(ShellExecutor),
                compare_files.clone(),
            ));
        }

        Ok(test)
    }
}
