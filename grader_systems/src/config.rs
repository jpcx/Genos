use std::{
    fs,
    path::{Path, PathBuf},
};

use async_trait::async_trait;
use genos::{
    gs::TestDescription,
    points::{PointQuantity, Points},
    stage::{compare_files::ComparesConfig, import_files::ImportConfig},
    tid::TestId,
};

use anyhow::Result;
use argh::FromArgs;
use serde::{de, Deserialize, Deserializer};
use thiserror::Error;
use tokio::{fs::File, io::AsyncReadExt};

use crate::stage::{compile::CompileConfig, run::RunConfig, valgrind::ValgrindConfig};

pub const TEST_CONFIG_NAME: &'static str = "config.yaml";

#[async_trait]
pub trait FromConfigFile {
    type Config;

    async fn from_file(path: &Path) -> Result<Self::Config>;
}

/// Config is the global config object which includes the config for the hw being run, all the
/// testcase configs which were found in the test resource directories and the config given through
/// the cli
pub struct Config {
    pub hw: HwConfig,
    pub cli: Cli,
    pub tests: Vec<TestConfig>,
}

#[derive(Debug, Deserialize)]
pub enum TestType {
    Diff,
}

#[derive(Deserialize)]
pub struct HwConfig {
    pub class: String,
    pub name: String,
    pub groups: Vec<TestGroup>,
}

#[async_trait]
impl FromConfigFile for HwConfig {
    type Config = HwConfig;

    async fn from_file(path: &Path) -> Result<Self::Config> {
        let mut file = File::open(path).await?;
        let mut contents = String::new();
        file.read_to_string(&mut contents).await?;
        Ok(serde_yaml::from_str(&contents)?)
    }
}

#[derive(Deserialize)]
pub struct TestGroup {
    pub name: String,
    pub tests: Vec<TestId>,
}

#[derive(Debug, Deserialize)]
#[serde(remote = "Self")]
pub struct TestConfig {
    pub description: TestDescription,
    pub test_type: TestType,
    pub compile: CompileConfig,
    pub run: RunConfig,
    pub compare_files: Option<ComparesConfig>,
    pub import_files: Option<ImportConfig>,
    pub valgrind: Option<ValgrindConfig>,
}

#[async_trait]
impl FromConfigFile for TestConfig {
    type Config = TestConfig;

    async fn from_file(path: &Path) -> Result<Self::Config> {
        let mut file = File::open(path).await?;
        let mut contents = String::new();
        file.read_to_string(&mut contents).await?;
        Ok(serde_yaml::from_str(&contents)?)
    }
}

impl TestConfig {
    fn validate(&self) -> Result<(), TestConfigValidationError> {
        let mut configured_points = Vec::new();

        // GRADERS: Other fields in the test case config will require adding code here to do
        // validation, add a lines here to add the points in the config to he configured_points
        // vector.

        if let Some(rc_config) = &self.run.return_code {
            configured_points.push(rc_config.points);
        }

        if let Some(compare_config) = &self.compare_files {
            configured_points.extend(compare_config.compares.iter().map(|compare| compare.points));
        }

        // GRADERS: Add to configured_points above when adding a new type to the config which has
        // points assigned to it.

        let all_full_points = configured_points.iter().all(|p| p.is_full_points());
        let all_partial_points = configured_points.iter().all(|p| !p.is_full_points());

        if !(all_full_points || all_partial_points) {
            return Err(TestConfigValidationError::MixedPointQuantities);
        }

        if all_partial_points {
            let total = configured_points
                .iter()
                .map(|p| match p {
                    PointQuantity::Partial(points) => points,
                    PointQuantity::FullPoints => unreachable!(),
                })
                .fold(Points::default(), |acc, curr| acc + *curr);

            if total != self.description.total_points {
                return Err(TestConfigValidationError::InvalidPointTotal {
                    configured_total_points: self.description.total_points,
                    calculated_total_points: total,
                });
            }
        }

        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum TestConfigValidationError {
    #[error("Points can only be all FullPoints or all Partial Points.")]
    MixedPointQuantities,

    #[error("Configured points need to add up to the total points. Configured total: {configured_total_points}, Calculated total: {calculated_total_points}")]
    InvalidPointTotal {
        configured_total_points: Points,
        calculated_total_points: Points,
    },
}

// Add a custom deserializer to verify that the test config is valid.
impl<'de> Deserialize<'de> for TestConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let this = Self::deserialize(deserializer)?;
        // validate that for each portion of the test case which has points associated with it, the
        // points are either all Partial(N) or all FullPoints. Also validate that if all points are
        // Partial, that the contained point value add up to the total_points in the test
        // description.
        this.validate().map_err(de::Error::custom)?;

        Ok(this)
    }
}

#[derive(FromArgs)]
/// Run the autograder for the systems course
pub struct Cli {
    /// path to the hw config
    #[argh(option, short = 'h', from_str_fn(make_absolute))]
    pub config: PathBuf,

    /// path to the submission to run
    #[argh(option, short = 's', from_str_fn(make_absolute))]
    pub submission: PathBuf,

    /// test grouping to run, must be a named group in the hw config
    #[argh(option, short = 'g')]
    pub group: Option<String>,
}

fn make_absolute(path_arg: &str) -> Result<PathBuf, String> {
    let path = fs::canonicalize(&path_arg)
        .map_err(|e| format!("error creating absolute path from {path_arg}: {e}"))?;

    if !path.exists() {
        return Err(format!("{} does not exist", path_arg));
    }

    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_hw_config() {
        serde_yaml::from_str::<HwConfig>(
            r#"
            class: 2022-winter
            name: hw1
            groups:
                -
                    name: brians-tests
                    tests: [1, 2, 3]
                -
                    name: ec-tests
                    tests:
                        - 4
                        - 5
                        - 6
            "#,
        )
        .unwrap();
    }

    #[test]
    fn deserialize_testcase_config() {
        serde_yaml::from_str::<TestConfig>(
            r#"
            description:
                name: test 1
                description: test 1
                test_id: 1
                total_points: 4
                visibility: Hidden
                tags: [tag 1, tag2]
                
            test_type: Diff
            
            compile:
                make_args: [arg1, arg 2, arg3]
                
            run:
                args: [arg1]
                executable: exec/bin
                timeout_sec: 30
                stdout: stdout_file
                stderr: stderr_file
                stdin: stdin_file
                return_code:
                    expected: 3
                    points: !Partial 0.25
                    
            compare_files:
                compares:
                    - 
                        expected: [expected_stdout, alternate]
                        student_file: student_out
                        compare_type: Grep
                        points: !Partial 2.25
                        show_output: true
                    -
                        expected: [expected_stderr, alternate]
                        student_file: student_stderr
                        compare_type: Diff
                        points: !Partial 1.5
                        show_output: false
                
            import_files:
                files: ["file 1", file 2]
            "#,
        )
        .unwrap();
    }

    #[test]
    fn deserialize_test_config_points_dont_add_up_to_total() {
        serde_yaml::from_str::<TestConfig>(
            r#"
            description:
                name: test 1
                description: test 1
                test_id: 1
                points: 4.0
                visibility: Hidden
                tags: [tag 1, tag2]
                
            test_type: Diff
            
            compile:
                make_args: [arg1, arg 2, arg3]
                
            run:
                args: [arg1]
                executable: exec/bin
                timeout_sec: 30
                stdout: stdout_file
                stderr: stderr_file
                stdin: stdin_file
                return_code:
                    expected: 3
                    points: !Partial 1
                    
            compare_files:
                compares:
                    - 
                        expected: [expected_stdout, alternate]
                        student_file: student_out
                        compare_type: Grep
                        points: !Partial 1
                        show_output: true
                    -
                        expected: [expected_stderr, alternate]
                        student_file: student_stderr
                        compare_type: Diff
                        points: !Partial 1
                        show_output: false
            "#,
        )
        .unwrap_err();
    }

    #[test]
    fn deserialize_test_config_point_types_mixed() {
        serde_yaml::from_str::<TestConfig>(
            r#"
            description:
                name: test 1
                description: test 1
                test_id: 1
                points: 4.0
                visibility: Hidden
                tags: [tag 1, tag2]
                
            test_type: Diff
            
            compile:
                make_args: [arg1, arg 2, arg3]
                
            run:
                args: [arg1]
                executable: exec/bin
                timeout_sec: 30
                stdout: stdout_file
                stderr: stderr_file
                stdin: stdin_file
                return_code:
                    expected: 3
                    points: !FullPoints
                    
            compare_files:
                compares:
                    - 
                        expected: [expected_stdout, alternate]
                        student_file: student_out
                        compare_type: Grep
                        points: !Partial 1
                        show_output: true
            "#,
        )
        .unwrap_err();
    }
}
