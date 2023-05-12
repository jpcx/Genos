use std::path::Path;

use anyhow::Result;
use async_trait::async_trait;
use tracing::{debug, instrument};

use crate::{
    output::Output,
    points::{PointQuantity, Points},
    score::Score,
    stage::{StageResult, StageStatus},
    tid::TestId,
    Executor,
};

#[derive(Debug, Eq, PartialEq)]
pub enum TestStatus {
    Pass(Score),
    Fail(Score),
}

impl TestStatus {
    pub fn possible_points(&self) -> Points {
        self.score().possible()
    }

    pub fn score(&self) -> Score {
        match self {
            Self::Pass(score) | Self::Fail(score) => *score,
        }
    }
}

impl Into<TestResult> for TestStatus {
    fn into(self) -> TestResult {
        TestResult {
            status: self,
            output: Output::default(),
        }
    }
}

pub struct TestResult {
    pub status: TestStatus,
    pub output: Output,
}

impl std::fmt::Debug for TestResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TestResult")
            .field("status", &self.status)
            .finish()
    }
}

impl TestResult {
    pub fn new(max_points: Points) -> Self {
        Self {
            status: TestStatus::Pass(Score::full_points(max_points)),
            output: Output::default(),
        }
    }

    fn append_stage_result(&mut self, res: StageResult) {
        self.output.append(res.output.unwrap_or_default());
        match res.status {
            StageStatus::Continue { points_lost } => match points_lost {
                PointQuantity::FullPoints => self.lose_full_points(),
                PointQuantity::Partial(points) => self.subtract_points(points),
            },
            StageStatus::UnrecoverableFailure => self.lose_full_points(),
        }
    }

    fn lose_full_points(&mut self) {
        let max_points = self.status.possible_points();
        self.status = TestStatus::Fail(Score::zero_points(max_points));
    }

    fn subtract_points(&mut self, points: Points) {
        if points != Points::new(0) {
            let new_score = self.status.score().remove_points(points);
            self.status = TestStatus::Fail(new_score);
        }
    }
}

#[async_trait]
pub trait Test: Executor<Output = TestResult> {
    /// the test id for this testcase
    fn id(&self) -> TestId;

    /// the number of points this test is worth
    fn points(&self) -> Points;
}

/// GenosTest is an opinionated executor for a series of test stages. It can be built with a number
/// of stages. When run, it will execute the stages in the order which they were given. Each test
/// stage is expected to know how to execute its own stage without any more information than where
/// to execute.
pub struct GenosTest {
    tid: TestId,
    points: Points,
    stages: Vec<Box<dyn Executor<Output = StageResult>>>,
}

impl GenosTest {
    pub fn new(tid: TestId, points: Points) -> Self {
        Self {
            tid,
            points,
            stages: Vec::new(),
        }
    }

    pub fn stage(mut self, stage: impl Executor<Output = StageResult> + 'static) -> Self {
        self.stages.push(Box::new(stage));
        self
    }

    pub fn add_stage(&mut self, stage: impl Executor<Output = StageResult> + 'static) {
        self.stages.push(Box::new(stage));
    }

    pub fn stages<I, E>(mut self, stages: I) -> Self
    where
        I: IntoIterator<Item = E>,
        E: Executor<Output = StageResult> + 'static,
    {
        for stage in stages {
            self.stages.push(Box::new(stage));
        }
        self
    }
}

/// GenosTest will go through and run each stage and collate the results into something which can
/// be interpreted by the results writers.
/// When a stage returns StageStatus::UnrecoverableFailure (such as during a compilation error or a
/// run error), then the test will not continue, and will result in an automatic failure.
/// When a stage Returns a StageStatus::Continue, the StagePoints will be recorded and the test
/// will continue with the next stage, and so on until an unrecoverable failure is reached, or the
/// test is completed.
///
/// At the end, it will process the StagePoints to determine the final score. There could be a
/// mixture of StagePoints::Absolute and StagePoints::Partial indicating the score for each stage.
///
/// If there was ever a StagePoints::Absolute(false) indicating the stage was worth a pass/fail and
/// failed, then the entire test will be marked as failed and the full points will be deducted
/// from the total possible points. In this case, the full output will still be showed to the
/// student since we should always communicate the maximum amount of feedback possible.
///
/// If there was a mixture of Absolute and Partial and all Absolute stages were marked as pass,
/// then the partial points will all be summed and the student will be awarded the score as the sum
/// of points received over the total points possible for the test. It is expected that if the
/// stage awards partial points, then all stages worth partial points will sum to the total
/// possible points for this testcase.
///
/// The testcase is marked as a Fail if either
/// a) A stage returned StageStatus::UnrecoverableFailure indicating something went wrong with the
/// student code and the test could not continue.
/// b) A stage returned StageStatus::Continue(StagePoints::Absolute(false)) indicating a pass/fail
/// stage failed.
/// c) A stage resulted in the student losing points via only receiving partial points for a test
///
/// The testcase is marked as Pass if full points were received for the testcase.
#[async_trait]
impl Executor for GenosTest {
    type Output = TestResult;

    #[instrument(skip(self), tid = self.tid)]
    async fn run(&self, ws: &Path) -> Result<TestResult> {
        let mut test_result = TestResult::new(self.points());

        for stage in &self.stages {
            let res = stage.run(ws).await?;
            debug!(?res.status, "stage completed");
            let status = res.status;
            test_result.append_stage_result(res);

            if let StageStatus::UnrecoverableFailure = status {
                break;
            }
        }

        Ok(test_result)
    }
}

#[async_trait]
impl Test for GenosTest {
    fn id(&self) -> TestId {
        self.tid
    }
    fn points(&self) -> Points {
        self.points
    }
}

#[cfg(test)]
mod tests {
    use std::{
        path::PathBuf,
        sync::{
            atomic::{AtomicU32, Ordering},
            Arc,
        },
    };

    use super::*;
    use anyhow::anyhow;

    #[test]
    fn create_genos_test() {
        struct MockStage;
        #[async_trait]
        impl Executor for MockStage {
            type Output = StageResult;
            async fn run(&self, _ws: &Path) -> Result<StageResult> {
                Ok(StageResult::new_continue(PointQuantity::zero()))
            }
        }

        let _test = GenosTest::new(TestId::new(0), Points::new(0.0))
            .stage(MockStage)
            .stage(MockStage);
    }

    struct MockStage {
        res: Result<StageResult>,
        call_count: Arc<AtomicU32>,
    }

    #[async_trait]
    impl Executor for MockStage {
        type Output = StageResult;

        async fn run(&self, _ws: &Path) -> Result<StageResult> {
            self.call_count.fetch_add(1, Ordering::Relaxed);
            match &self.res {
                Ok(res) => Ok(res.clone()),
                Err(e) => Err(anyhow!("stage error: {:?}", e)),
            }
        }
    }

    fn get_stage_list_with_results<I: IntoIterator<Item = Result<StageResult>>>(
        list: I,
    ) -> Vec<MockStage> {
        list.into_iter()
            .map(|res| MockStage {
                res,
                call_count: Arc::default(),
            })
            .collect()
    }

    #[tokio::test]
    async fn genos_runs_all_on_success() {
        let stages = get_stage_list_with_results([
            Ok(StageResult::new_continue(PointQuantity::zero())),
            Ok(StageResult::new_continue(PointQuantity::zero())),
        ]);

        let test = GenosTest::new(TestId::new(0), Points::new(4)).stages(stages);
        let res = test.run(&PathBuf::new()).await.unwrap();
        assert_eq!(res.status, TestStatus::Pass(Score::new(4, 4)));
    }

    #[tokio::test]
    async fn genos_stops_on_first_fail() {
        let stages = get_stage_list_with_results([
            Ok(StageResult::new_continue(PointQuantity::zero())),
            Ok(StageResult::new_unrecoverable_failure()),
            Ok(StageResult::new_continue(PointQuantity::zero())),
        ]);

        let last_stage_count = stages[2].call_count.clone();

        let test = GenosTest::new(TestId::new(0), Points::new(4)).stages(stages);

        let res = test.run(&PathBuf::new()).await.unwrap();
        assert!(matches!(res.status, TestStatus::Fail(_)));
        assert_eq!(last_stage_count.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn genos_stops_on_first_error() {
        let stages = get_stage_list_with_results([
            Ok(StageResult::new_continue(PointQuantity::zero())),
            Err(anyhow!("stage error")),
            Ok(StageResult::new_continue(PointQuantity::zero())),
        ]);

        let last_stage_count = stages[2].call_count.clone();

        let test = GenosTest::new(TestId::new(0), Points::new(4)).stages(stages);

        test.run(&PathBuf::new()).await.unwrap_err();
        assert_eq!(last_stage_count.load(Ordering::Relaxed), 0);
    }
}
