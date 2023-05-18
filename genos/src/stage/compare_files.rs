use std::{
    fmt::Display,
    path::{Path, PathBuf},
};

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use serde::Deserialize;
use tokio::{fs::File, io::AsyncReadExt};
use tracing::debug;

use crate::{
    fs::{filename, filepath, ResourceLocatorCreator},
    output::{self, Content, Output, RichTextMaker, Section, StatusUpdates, Update},
    points::PointQuantity,
    process::{Command, ExitStatus, ProcessExecutor},
    stage::StageResult,
    Executor,
};

#[derive(Debug, Deserialize, Clone)]
pub struct ComparesConfig {
    pub compares: Vec<CompareConfig>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct CompareConfig {
    pub expected: Vec<String>,
    pub student_file: String,
    pub compare_type: CompareType,
    pub points: PointQuantity,
    pub show_output: bool,
}

#[derive(Debug, Eq, PartialEq, Deserialize, Clone)]
pub enum CompareType {
    Diff,
    Grep,
    ReverseGrep,
}

impl Display for CompareType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Diff => "diff",
            Self::Grep => "grep",
            Self::ReverseGrep => "reverse grep",
        };

        write!(f, "{}", s)
    }
}

#[async_trait]
pub trait Comparator: Send + Sync {
    async fn compare(&self, file1: &Path, file2: &Path) -> Result<bool>;
}

pub trait ComparatorCreator: Send + Sync {
    fn create(&self, ctype: &CompareType) -> Box<dyn Comparator>;
}

pub struct ComparatorCreatorImpl<E> {
    executor: E,
}

impl<E: ProcessExecutor + 'static> ComparatorCreatorImpl<E> {
    pub fn new(executor: E) -> Self {
        Self { executor }
    }
}

impl<E: ProcessExecutor + 'static> ComparatorCreator for ComparatorCreatorImpl<E> {
    fn create(&self, ctype: &CompareType) -> Box<dyn Comparator> {
        match ctype {
            CompareType::Diff => Box::new(DiffCompare {
                executor: self.executor.clone(),
            }),
            _ => panic!(),
        }
    }
}

pub struct DiffCompare<E> {
    executor: E,
}

impl<E: ProcessExecutor> DiffCompare<E> {
    pub fn new(executor: E) -> Self {
        Self { executor }
    }
}

#[async_trait]
impl<E: ProcessExecutor + 'static> Comparator for DiffCompare<E> {
    async fn compare(&self, file1: &Path, file2: &Path) -> Result<bool> {
        let res = Command::new("cmp")
            .args([filepath(file1)?, filepath(file2)?])
            .run_with(&self.executor)
            .await?;

        match res.status {
            ExitStatus::Ok => Ok(true),
            ExitStatus::Failure(code) => {
                // cmp returns 2+ if encountered error
                if code > 1 {
                    Err(anyhow!("Error running cmp"))
                } else {
                    Ok(false)
                }
            }
            _ => Err(anyhow!("Error running cmp: {}", res.stderr)),
        }
    }
}

pub struct CompareFiles<F, C> {
    // fs_creator can create a resource resolver based on the ws. We can't simply use a normal
    // resolver here since depending on the test type, we may need to look in the ws which is not known
    // until the stage is run. For example, a normal ResourceLocator would know to look in the test
    // resource directory or static, etc for expected output files. But in some tests like the
    // segregated allocator, the correct output is generated at runtime in the ws folder so the ws
    // folder needs to be added to the list of directories to search in.
    fs_creator: F,
    comparator_creator: C,
    config: ComparesConfig,
}

impl<F, C> CompareFiles<F, C>
where
    F: ResourceLocatorCreator,
    C: ComparatorCreator,
{
    pub fn new(fs_creator: F, comparator_creator: C, config: ComparesConfig) -> Self {
        Self {
            fs_creator,
            comparator_creator,
            config,
        }
    }

    async fn match_any(&self, compare: &CompareConfig, ws: &Path) -> Result<bool> {
        // fs knows where to locate any expected resource files
        let finder = self.fs_creator.create(ws);
        let comparator = self.comparator_creator.create(&compare.compare_type);
        let student_file = ws.join(&compare.student_file);

        for expected_file_name in &compare.expected {
            let expected_file_path = finder.find(expected_file_name)?;
            if comparator
                .compare(&expected_file_path, &student_file)
                .await?
            {
                return Ok(true);
            }
        }

        Ok(false)
    }

    // The output for compare stage should look something like
    // [ Compare Output ]
    //
    // Comparing stdout.txt (diff) ... pass
    // Comparing stderr.txt (diff) ... pass
    // Comparing file1.txt (diff) .... fail (-2)
    // Comparing file2.txt (diff) .... fail (-2)
    //
    // -- Expected file1.txt
    // ```
    // 01| line 1
    // 02| line 2
    // ```
    //
    // -- Actual file1.txt
    // ```
    // 01| line 1
    // 02| line 3
    // ```
    //
    // -- Expected file2.txt
    // ```
    // 01| line 1
    // 02| line 2
    // ```
    //
    // -- Actual file2.txt
    // ```
    // 01| line 1
    // 02| line 2
    // 03| extra line
    // ```
    async fn get_failed_compare_feedback(
        &self,
        compare: &CompareConfig,
        expected_file: &PathBuf,
        student_file: &PathBuf,
    ) -> Result<output::Content> {
        if !compare.show_output {
            let filename = filename(student_file)?;
            let lines = [
                format!("Actual {} did not match expected {}", filename, filename,),
                "The instructor has chosen to keep this output hidden.".to_string(),
            ];
            return Ok(lines.join("\n").into());
        }

        match &compare.compare_type {
            CompareType::Diff => {
                self.get_failed_diff_feedback(expected_file, student_file)
                    .await
            }
            _ => panic!(),
        }
    }

    async fn get_failed_diff_feedback(
        &self,
        expected_file: &PathBuf,
        student_file: &PathBuf,
    ) -> Result<output::Content> {
        let expected_content = load_transformed_content(expected_file).await?.code();
        let student_content = load_transformed_content(student_file).await?.code();

        let expected_section = Content::SubSection(
            Section::new(format!("Expected {}", filename(student_file)?)).content(expected_content),
        );

        let actual_section = Content::SubSection(
            Section::new(format!("Actual {}", filename(student_file)?)).content(student_content),
        );

        Ok(Content::Multiline(
            [expected_section, actual_section].to_vec(),
        ))
    }
}

#[async_trait]
impl<F, C> Executor for CompareFiles<F, C>
where
    F: ResourceLocatorCreator + Send + Sync,
    C: ComparatorCreator,
{
    type Output = StageResult;

    async fn run(&self, ws: &Path) -> Result<Self::Output> {
        let mut compare_status_updates = StatusUpdates::default();
        let mut points_lost = PointQuantity::zero();

        for compare_config in &self.config.compares {
            debug!("Running compare {:?}", compare_config);
            let mut update = Update::new_pass(format!(
                "Comparing {} ({})",
                compare_config.student_file, compare_config.compare_type
            ));
            let student_file = ws.join(&compare_config.student_file);

            // first check to see if the student file exists
            if !student_file.exists() {
                debug!("Could not find student file: {}", student_file.display());
                update.set_fail(compare_config.points);
                update.set_notes(format!(
                    "Could not find file {} in root of workspace",
                    compare_config.student_file
                ));
                compare_status_updates.add_update(update);

                points_lost += compare_config.points;

                continue;
            }

            // if the file exists, then run the compare. Get the correct comparator from the
            // comparator factory.
            if self.match_any(compare_config, ws).await? {
                compare_status_updates.add_update(update);
                continue;
            }

            // if we didn't find a match, then we need to give the student feedback
            update.set_fail(compare_config.points);
            points_lost += compare_config.points;

            let finder = self.fs_creator.create(ws);
            let expected_file = finder.find(&compare_config.expected[0])?;
            update.set_notes(
                self.get_failed_compare_feedback(&compare_config, &expected_file, &student_file)
                    .await?,
            );

            compare_status_updates.add_update(update);
        }

        let output =
            Output::new().section(Section::new("Compare Output").content(compare_status_updates));

        Ok(StageResult::new_continue(points_lost).with_output(output))
    }
}

async fn load_transformed_content(file: &PathBuf) -> Result<String> {
    let mut file = File::open(file).await?;
    let mut contents = Vec::new();
    file.read_to_end(&mut contents).await?;

    let mut res = String::new();
    let mut line_number: u64 = 1;

    let transform_byte = |byte: &u8| -> String {
        let transformed = match byte {
            0 => "(\\x00)",
            9..=13 => match byte {
                9 => "(\\t)",
                10 => "\\n\n",
                11 => "(\\v)",
                12 => "(\\f)",
                13 => "(\\r)",
                _ => unreachable!(),
            },
            32..=126 => return std::str::from_utf8(&[*byte]).unwrap().to_string(),
            _ => return format!("({:#02x})", byte),
        };

        transformed.to_string()
    };

    let mut line_number_str = || -> String {
        let s = format!("{:02}| ", line_number);
        line_number += 1;
        s
    };

    res.push_str(line_number_str().as_str());
    for byte in &contents {
        res.push_str(transform_byte(byte).as_str());
        if char::from(*byte) == '\n' {
            res.push_str(line_number_str().as_str());
        }
    }

    Ok(res)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use crate::{
        fs::ResourceLocator,
        output::Contains,
        points::Points,
        process::{self, ShellExecutor},
        stage::StageStatus,
        test_util::{create_temp_file_in, MockDir, MockExecutorInner, MockProcessExecutor},
    };

    use super::*;

    #[tokio::test]
    async fn transformed_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = create_temp_file_in(&dir, "test", &[0, 9, 10, 11, 12, 13, 32, 5]);
        let content = load_transformed_content(&path).await.unwrap();
        let expected = r#"01| (\x00)(\t)\n
02| (\v)(\f)(\r) (0x5)"#;
        assert_eq!(&content, expected);
    }

    #[tokio::test]
    async fn file_not_found_fails_compare() {
        let ws = MockDir::new();
        let finder_creator = |_ws: &Path| -> Box<dyn ResourceLocator> { Box::new(MockDir::new()) };
        let compares = ComparesConfig {
            compares: vec![CompareConfig {
                expected: vec!["expected_stdout".to_string()],
                student_file: "stdout".to_string(),
                compare_type: CompareType::Diff,
                points: PointQuantity::Partial(Points::new(4)),
                show_output: true,
            }],
        };

        let comparator_creator = ComparatorCreatorImpl::new(ShellExecutor);

        let stage = CompareFiles::new(finder_creator, comparator_creator, compares);
        let res = stage.run(&ws.root.path()).await.unwrap();
        assert_eq!(
            res.status,
            StageStatus::Continue {
                points_lost: PointQuantity::Partial(Points::new(4))
            }
        );

        assert!(res.output.unwrap().contains("Could not find file"));
    }

    #[tokio::test]
    async fn diff_compare_pass() {
        let finder_creator = |_ws: &Path| -> Box<dyn ResourceLocator> {
            Box::new(
                MockDir::new()
                    .file(("expected_stdout", "stdout here"))
                    .file(("expected_stderr", "stderr here")),
            )
        };
        let ws = MockDir::new()
            .file(("stdout", "stdout here"))
            .file(("stderr", "stderr here"));

        let compares = ComparesConfig {
            compares: vec![
                CompareConfig {
                    expected: vec!["expected_stdout".to_string()],
                    student_file: "stdout".to_string(),
                    compare_type: CompareType::Diff,
                    points: PointQuantity::Partial(Points::new(1)),
                    show_output: true,
                },
                CompareConfig {
                    expected: vec!["expected_stderr".to_string()],
                    student_file: "stderr".to_string(),
                    compare_type: CompareType::Diff,
                    points: PointQuantity::Partial(Points::new(2)),
                    show_output: true,
                },
            ],
        };

        let data = Arc::new(Mutex::new(MockExecutorInner::with_responses([
            Ok(process::Output::from_exit_status(ExitStatus::Ok)),
            Ok(process::Output::from_exit_status(ExitStatus::Ok)),
        ])));

        let comparator_creator = ComparatorCreatorImpl::new(MockProcessExecutor::new(data.clone()));

        let stage = CompareFiles::new(finder_creator, comparator_creator, compares);
        let res = stage.run(&ws.root.path()).await.unwrap();

        assert_eq!(
            res.status,
            StageStatus::Continue {
                points_lost: PointQuantity::zero(),
            }
        );
    }

    #[tokio::test]
    async fn diff_compare_absolute_fail() {
        let finder_creator = |_ws: &Path| -> Box<dyn ResourceLocator> {
            Box::new(
                MockDir::new()
                    .file(("expected_stdout", "stdout here"))
                    .file(("expected_stderr", "stderr here")),
            )
        };
        let ws = MockDir::new()
            .file(("stdout", "stdout here"))
            .file(("stderr", "stderr here"));

        let compares = ComparesConfig {
            compares: vec![
                CompareConfig {
                    expected: vec!["expected_stdout".to_string()],
                    student_file: "stdout".to_string(),
                    compare_type: CompareType::Diff,
                    points: PointQuantity::FullPoints,
                    show_output: true,
                },
                CompareConfig {
                    expected: vec!["expected_stderr".to_string()],
                    student_file: "stderr".to_string(),
                    compare_type: CompareType::Diff,
                    points: PointQuantity::FullPoints,
                    show_output: true,
                },
            ],
        };

        let data = Arc::new(Mutex::new(MockExecutorInner::with_responses([
            Ok(process::Output::from_exit_status(ExitStatus::Ok)),
            Ok(process::Output::from_exit_status(ExitStatus::Failure(1))),
        ])));

        let comparator_creator = ComparatorCreatorImpl::new(MockProcessExecutor::new(data.clone()));

        let stage = CompareFiles::new(finder_creator, comparator_creator, compares);
        let res = stage.run(&ws.root.path()).await.unwrap();

        assert_eq!(
            res.status,
            StageStatus::Continue {
                points_lost: PointQuantity::FullPoints,
            }
        );
    }

    #[tokio::test]
    async fn diff_compare_fail() {
        let finder_creator = |_ws: &Path| -> Box<dyn ResourceLocator> {
            Box::new(
                MockDir::new()
                    .file(("expected_stdout", "stdout here"))
                    .file(("expected_stderr", "stderr here")),
            )
        };
        let ws = MockDir::new()
            .file(("stdout", "stdout here"))
            .file(("stderr", "stderr here"));

        let compares = ComparesConfig {
            compares: vec![
                CompareConfig {
                    expected: vec!["expected_stdout".to_string()],
                    student_file: "stdout".to_string(),
                    compare_type: CompareType::Diff,
                    points: PointQuantity::Partial(Points::new(1)),
                    show_output: true,
                },
                CompareConfig {
                    expected: vec!["expected_stderr".to_string()],
                    student_file: "stderr".to_string(),
                    compare_type: CompareType::Diff,
                    points: PointQuantity::Partial(Points::new(2)),
                    show_output: true,
                },
            ],
        };

        let data = Arc::new(Mutex::new(MockExecutorInner::with_responses([
            Ok(process::Output::from_exit_status(ExitStatus::Ok)),
            Ok(process::Output::from_exit_status(ExitStatus::Failure(1))),
        ])));

        let comparator_creator = ComparatorCreatorImpl::new(MockProcessExecutor::new(data.clone()));

        let stage = CompareFiles::new(finder_creator, comparator_creator, compares);
        let res = stage.run(&ws.root.path()).await.unwrap();

        assert_eq!(
            res.status,
            StageStatus::Continue {
                points_lost: PointQuantity::Partial(Points::new(2)),
            }
        );
    }

    #[tokio::test]
    async fn diff_compare_tries_secondary_expected() {
        let finder_creator = |_ws: &Path| -> Box<dyn ResourceLocator> {
            Box::new(
                MockDir::new()
                    .file(("expected_stdout", "stdout here"))
                    .file(("expected_stdout2", "stdout here"))
                    .file(("expected_stderr", "stderr here")),
            )
        };
        let ws = MockDir::new()
            .file(("stdout", "stdout here"))
            .file(("stderr", "stderr here"));

        let compares = ComparesConfig {
            compares: vec![CompareConfig {
                expected: vec![
                    "expected_stdout".to_string(),
                    "expected_stdout2".to_string(),
                ],
                student_file: "stdout".to_string(),
                compare_type: CompareType::Diff,
                points: PointQuantity::Partial(Points::new(1)),
                show_output: true,
            }],
        };

        let data = Arc::new(Mutex::new(MockExecutorInner::with_responses([
            Ok(process::Output::from_exit_status(ExitStatus::Failure(1))),
            Ok(process::Output::from_exit_status(ExitStatus::Ok)),
        ])));

        let comparator_creator = ComparatorCreatorImpl::new(MockProcessExecutor::new(data.clone()));

        let stage = CompareFiles::new(finder_creator, comparator_creator, compares);
        let res = stage.run(&ws.root.path()).await.unwrap();

        assert_eq!(
            res.status,
            StageStatus::Continue {
                points_lost: PointQuantity::zero(),
            }
        );
    }

    #[tokio::test]
    async fn diff_comparator_pass() {
        let dir = tempfile::tempdir().unwrap();
        let file1 = create_temp_file_in(&dir, "file1", "contents");
        let file2 = create_temp_file_in(&dir, "file2", "contents");

        let comparator = DiffCompare::new(ShellExecutor);
        let is_match = comparator
            .compare(file1.as_path(), file2.as_path())
            .await
            .unwrap();
        assert!(is_match);
    }

    #[tokio::test]
    async fn diff_comparator_fail() {
        let dir = tempfile::tempdir().unwrap();
        let file1 = create_temp_file_in(&dir, "file1", "contents");
        let file2 = create_temp_file_in(&dir, "file2", "contents1");

        let comparator = DiffCompare::new(ShellExecutor);
        let is_match = comparator
            .compare(file1.as_path(), file2.as_path())
            .await
            .unwrap();
        assert!(!is_match);
    }
}
