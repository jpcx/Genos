use std::path::Path;

use anyhow::Result;
use async_trait::async_trait;

use crate::{output::Output, points::PointQuantity, Executor};

pub mod compare_files;
pub mod import_files;

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

    /// Implement Executor for anything implementing a SystemStageExecutor. SystemStageExecutor
    /// returns either OK or an error. If it returned an error, then bubble that up, otherwise map
    /// an OK value to a StageResult.
    async fn run(&self, ws: &Path) -> Result<Self::Output> {
        self.run(ws)
            .await
            .map(|_| StageResult::new_continue(PointQuantity::zero()))
    }
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

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum StageStatus {
    Continue { points_lost: PointQuantity },
    UnrecoverableFailure,
}

impl StageStatus {}

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

impl StageResult {
    pub fn new(status: StageStatus, output: Option<Output>) -> Self {
        Self { status, output }
    }

    pub fn new_unrecoverable_failure() -> Self {
        Self::new(StageStatus::UnrecoverableFailure, None)
    }

    pub fn new_continue(points_lost: PointQuantity) -> Self {
        Self::new(StageStatus::Continue { points_lost }, None)
    }

    pub fn with_output(mut self, output: Output) -> Self {
        self.output = Some(output);
        self
    }
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
            StageStatus::Continue {
                points_lost: PointQuantity::zero(),
            }
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
