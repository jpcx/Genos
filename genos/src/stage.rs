use std::path::Path;

use anyhow::Result;
use async_trait::async_trait;

use crate::{output::Output, score::Score, Executor};

pub mod compare_files;
pub mod import_files;
pub mod valgrind;

/// System Stage executors represent executors which are for the autograding system itself. As
/// such, they cannot by themselves fail a test. Any errors by a system stage should be treated as
/// an error with the autograder.
#[async_trait]
pub trait SystemStageExecutor: Send + Sync {
    async fn run(&self, ws: &Path) -> Result<()>;
}

#[async_trait]
impl<T> Executor for T
where
    T: SystemStageExecutor,
{
    type Output = StageResult;
    async fn run(&self, ws: &Path) -> Result<Self::Output> {
        self.run(ws).await.map(|_| StageResult {
            status: StageStatus::Continue(StagePoints::Absolute(true)),
            output: None,
        })
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum StagePoints {
    /// Designates the stage was pass/fail along with the pass status
    Absolute(bool),

    /// Disignates the stage as counting towards partial credit, and the score received
    Partial(Score),
}

/// Each stage needs to communicate
/// - failure due to system error (return Err)
/// - Unrecoverable error (Compilation failure, Could not run assignment)
/// - Continue with awarded points
///
/// - When a stage rewards points, it can either award partial points, or it can pass/fail
///   absolutely.
/// - Within a single testcase, it's expected to have one or the other, either all stages which can
///   award points are pass/fail, or award partial points
/// - Each stage needs to be able to communicate the score to the test runner
///     - if pass/fail, then the testcase is marked as fail if any stage fails
///     - if partial, then the testcase will tally the number of received points vs awarded.
///     - In any case, the test will continue until all stages are completed or until an
///       unrecoverable failure is hit.

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum StageStatus {
    Continue(StagePoints),
    UnrecoverableFailure,
}

impl StageStatus {
    pub fn unwrap_points(self) -> StagePoints {
        match self {
            Self::Continue(points) => points,
            Self::UnrecoverableFailure => panic!("Expected variant with points"),
        }
    }
}

impl Into<StageResult> for StageStatus {
    fn into(self) -> StageResult {
        StageResult {
            status: self,
            output: None,
        }
    }
}

#[derive(Clone)]
pub struct StageResult {
    pub status: StageStatus,
    pub output: Option<Output>,
}

impl std::fmt::Debug for StageResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StageResult")
            .field("status", &self.status)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use anyhow::anyhow;

    struct SystemExecutor {
        res: Result<()>,
    }

    #[async_trait]
    impl SystemStageExecutor for SystemExecutor {
        async fn run(&self, _ws: &Path) -> Result<()> {
            match &self.res {
                Ok(_) => Ok(()),
                Err(e) => Err(anyhow!("Found error: {e}")),
            }
        }
    }

    #[tokio::test]
    async fn system_stage_success() {
        let exec = SystemExecutor { res: Ok(()) };
        let executor: Box<dyn Executor<Output = StageResult>> = Box::new(exec);
        let stage_result = executor.run(&PathBuf::new()).await.unwrap();
        assert_eq!(
            stage_result.status,
            StageStatus::Continue(StagePoints::Absolute(true))
        );
    }

    #[tokio::test]
    async fn system_stage_fail() {
        let exec = SystemExecutor {
            res: Err(anyhow!("got error")),
        };
        let executor: Box<dyn Executor<Output = StageResult>> = Box::new(exec);
        let _stage_result = executor.run(&PathBuf::new()).await.unwrap_err();
    }
}
