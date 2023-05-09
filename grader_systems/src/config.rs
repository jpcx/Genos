use genos::{
    points::Points,
    stage::{compare_files::ComparesConfig, import_files::ImportConfig},
    tid::TestId,
};

use crate::stage::{compile::CompileConfig, run::RunConfig};

pub enum TestType {
    Diff,
}

pub struct Config {
    pub hw_name: String,
    pub testcases: Vec<TestConfig>,
}

pub struct TestConfig {
    pub tid: TestId,
    pub test_type: TestType,
    pub points: Points,
    pub compile: CompileConfig,
    pub run: RunConfig,
    pub compare_files: Option<ComparesConfig>,
    pub import_files: Option<ImportConfig>,
}
