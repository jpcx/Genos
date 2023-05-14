use std::{fmt::Display, path::PathBuf};

use serde::{Deserialize, Serialize};

use crate::{points::Points, test, tid::TestId};

#[derive(Clone, Copy, Deserialize, Serialize)]
pub enum Visibility {
    Hidden,
    Visible,
    AfterDueDate,
    AfterPublished,
}

impl Display for Visibility {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Hidden => write!(f, "hidden"),
            Self::Visible => write!(f, "visible"),
            Self::AfterDueDate => write!(f, "after_due_date"),
            Self::AfterPublished => write!(f, "after_published"),
        }
    }
}

pub fn running_in_gs() -> bool {
    PathBuf::from("/autograder").exists()
}

pub trait Description {
    fn name(&self) -> String;
    fn description(&self) -> String;
    fn visibility(&self) -> Visibility;
    fn id(&self) -> TestId;
    fn tags(&self) -> Vec<String> {
        Vec::new()
    }
}

#[derive(Deserialize, Serialize)]
pub struct TestDescription {
    pub name: String,
    pub description: String,
    pub test_id: TestId,
    pub points: Points,
    pub visibility: Visibility,
    pub tags: Option<Vec<String>>,
}

impl Description for TestDescription {
    fn name(&self) -> String {
        self.name.clone()
    }

    fn description(&self) -> String {
        self.description.clone()
    }

    fn visibility(&self) -> Visibility {
        self.visibility
    }

    fn id(&self) -> TestId {
        self.test_id
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
pub enum TextFormat {
    Text,
    Markdown,
}

#[derive(Serialize)]
pub enum TestStatus {
    Passed,
    Failed,
}

impl Display for TestStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Passed => write!(f, "passed"),
            Self::Failed => write!(f, "failed"),
        }
    }
}

impl From<test::TestStatus> for TestStatus {
    fn from(value: test::TestStatus) -> Self {
        match value {
            test::TestStatus::Pass(_) => TestStatus::Passed,
            test::TestStatus::Fail(_) => TestStatus::Failed,
        }
    }
}

pub trait FormatType {
    fn format_type(&self) -> TextFormat;
}

#[derive(Serialize)]
pub struct Results {
    pub output_format: TextFormat,
    pub tests: Vec<TestResult>,
}

#[derive(Serialize)]
pub struct TestResult {
    pub score: Points,
    pub max_score: Points,
    pub status: TestStatus,
    pub name: String,
    pub output: String,
    pub tags: Vec<String>,
    pub visibility: Visibility,
}
