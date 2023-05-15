use std::path::{Path, PathBuf};

use anyhow::Result;
use config::{TestConfig, TestType};
use genos::{
    fs::{ResourceLocator, ResourceLocatorCreator},
    gs::running_in_gs,
    process::{is_program_in_path, ShellExecutor},
    stage::{
        compare_files::{ComparatorCreatorImpl, CompareFiles},
        import_files::ImportFiles,
    },
    test::GenosTest,
};
use stage::{compile::Compile, run::Run};

mod config;
mod stage;

struct TestResourceLocator;

impl ResourceLocator for TestResourceLocator {
    fn find(&self, name: &String) -> Result<PathBuf> {
        todo!();
    }
}

struct TestResourceLocatorCreator;

impl ResourceLocatorCreator for TestResourceLocatorCreator {
    fn create(&self, ws: &Path) -> Box<dyn ResourceLocator> {
        Box::new(TestResourceLocator)
    }
}

fn build_testcase(config: &TestConfig) -> Result<GenosTest> {
    match &config.test_type {
        TestType::Diff => {
            // test order should be
            // 1. import files (done)
            // 2. compile (done)
            // 3. run (done)
            // 4. compare (done)
            // 5. valgrind run
            // 6. run with memory limit
            let mut test = GenosTest::new(config.description.points);
            if let Some(import_files) = &config.import_files {
                test.add_stage(ImportFiles::new(import_files, &TestResourceLocator)?)
            }

            test.add_stage(Compile::new(&config.compile, ShellExecutor));

            test.add_stage(Run::new(ShellExecutor, config.run.clone()));

            let compare_files = config
                .compare_files
                .as_ref()
                .expect("Expected diff test to have at least one compare");

            test.add_stage(CompareFiles::new(
                TestResourceLocatorCreator,
                ComparatorCreatorImpl::new(ShellExecutor),
                compare_files.clone(),
            ));

            if let Some(_) = &config.valgrind {
                if running_in_gs() || is_program_in_path("valgrind") {
                    todo!();
                    //test.add_stage(Valgrind::new(...));
                } else {
                    tracing::warn!(
                        "cannot run valgrind stage on local instance \
                         without valgrind installed! skipping stage"
                    );
                }
            }

            Ok(test)
        }
    }
}

fn main() {
    println!("Hello, world!");
}
