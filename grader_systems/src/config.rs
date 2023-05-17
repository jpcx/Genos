use std::{fs, path::PathBuf};

use genos::{
    gs::TestDescription,
    stage::{compare_files::ComparesConfig, import_files::ImportConfig},
    tid::TestId,
};

use argh::FromArgs;

use crate::stage::{compile::CompileConfig, run::RunConfig};

pub enum TestType {
    Diff,
}

pub struct Config {
    pub hw_name: String,
    pub testcases: Vec<TestConfig>,
    // allow for named grouping of tests which all conform to a theme. For example, you could run
    // all tests marked as diff-test-short, or brians-tests
    pub groups: Vec<TestGroup>,
}

pub struct TestGroup {
    pub name: String,
    pub tests: Vec<TestId>,
}

pub struct TestConfig {
    pub description: TestDescription,
    pub test_type: TestType,
    pub compile: CompileConfig,
    pub run: RunConfig,
    pub compare_files: Option<ComparesConfig>,
    pub import_files: Option<ImportConfig>,
}

#[derive(FromArgs)]
/// Run the autograder for the systems course
pub struct Cli {
    /// where the data directory is located
    #[argh(option, short = 'd', from_str_fn(make_absolute))]
    data: PathBuf,

    /// what class offering the hw is in.
    #[argh(option, short = 'c')]
    class: String,

    /// what hw to run the autograder on
    #[argh(option, short = 'h')]
    hw: String,

    /// what submission to run
    #[argh(option, short = 's', from_str_fn(make_absolute))]
    submission: PathBuf,

    /// test grouping to run, must be a named group in the hw config
    #[argh(option, short = 'g')]
    group: Option<String>,
}

fn make_absolute(path_arg: &str) -> Result<PathBuf, String> {
    fs::canonicalize(&path_arg)
        .map_err(|e| format!("error creating absolute path from {path_arg}: {e}"))
}
