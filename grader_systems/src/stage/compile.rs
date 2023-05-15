use std::path::Path;

use anyhow::Result;
use async_trait::async_trait;
use genos::{
    output::{self, Content, RichTextMaker, Section, StatusUpdates, Update},
    points::PointQuantity,
    process::{self, Command, ExitStatus, ProcessExecutor},
    stage::StageResult,
    Executor,
};

#[derive(Default, Clone)]
pub struct CompileConfig {
    /// We always require a makefile to be present in the ws root. The args in the config are
    /// passed directly to make.
    /// Ex: "make"
    /// Ex: "make TEST=main.c"
    pub make_args: Option<Vec<String>>,
}

pub struct Compile<E> {
    args: Vec<String>,
    executor: E,
}

impl<E: ProcessExecutor> Compile<E> {
    pub fn new(config: &CompileConfig, executor: E) -> Self {
        let args = config.clone().make_args.unwrap_or(vec![]);
        Self { args, executor }
    }

    fn get_compile_feedback(&self, output: process::Output) -> Content {
        let stdout =
            Content::SubSection(Section::new("Compile Stdout").content(output.stdout.code()));
        let stderr =
            Content::SubSection(Section::new("Compile Stderr").content(output.stderr.code()));

        Content::Multiline([stdout, stderr].to_vec())
    }
}

#[async_trait]
impl<E: ProcessExecutor> Executor for Compile<E> {
    type Output = StageResult;
    async fn run(&self, ws: &Path) -> Result<Self::Output> {
        let mut section = Section::new("Compile");
        let mut status_updates = StatusUpdates::default();
        let mut update = Update::new_pass("Compiling submission");

        let output = Command::new("make")
            .args(self.args.clone())
            .cwd(ws)
            .run_with(&self.executor)
            .await?;

        match &output.status {
            ExitStatus::Ok => {
                status_updates.add_update(update);
                section.add_content(status_updates);

                Ok(StageResult::new_continue(PointQuantity::zero())
                    .with_output(output::Output::new().section(section)))
            }

            _ => {
                update.set_fail(PointQuantity::FullPoints);
                update.set_notes(self.get_compile_feedback(output));
                status_updates.add_update(update);

                section.add_content(status_updates);

                Ok(StageResult::new_unrecoverable_failure()
                    .with_output(output::Output::new().section(section)))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use genos::{
        output::Contains,
        stage::StageStatus,
        test_util::{MockExecutorInner, MockProcessExecutor},
    };

    use super::*;

    #[tokio::test]
    async fn compile_success() {
        let data = Arc::new(Mutex::new(MockExecutorInner::with_responses([Ok(
            process::Output::from_exit_status(ExitStatus::Ok),
        )])));

        let executor = MockProcessExecutor::new(data.clone());
        let config = CompileConfig {
            make_args: Some(vec!["arg1".to_string(), "arg2".to_string()]),
        };

        let compile = Compile::new(&config, executor);
        let ws = tempfile::tempdir().unwrap();
        let res = compile.run(ws.path()).await.unwrap();

        assert_eq!(
            res.status,
            StageStatus::Continue {
                points_lost: PointQuantity::zero()
            }
        );

        let cmd = data.lock().unwrap().commands.pop().unwrap();
        assert_eq!(&cmd.program, "make");
        assert_eq!(cmd.args, vec!["arg1".to_string(), "arg2".to_string()]);
        assert_eq!(cmd.cwd.unwrap().as_path(), ws.path());
    }

    #[tokio::test]
    async fn compile_failure() {
        let data = Arc::new(Mutex::new(MockExecutorInner::with_responses([Ok(
            process::Output::new(ExitStatus::Failure(2), "stdout here", "stderr also"),
        )])));

        let executor = MockProcessExecutor::new(data.clone());
        let config = CompileConfig {
            make_args: Some(vec!["arg1".to_string(), "arg2".to_string()]),
        };

        let compile = Compile::new(&config, executor);
        let ws = tempfile::tempdir().unwrap();
        let res = compile.run(ws.path()).await.unwrap();

        assert_eq!(res.status, StageStatus::UnrecoverableFailure);
        assert!(res.output.as_ref().unwrap().contains("stdout here"));
        assert!(res.output.as_ref().unwrap().contains("stderr also"));
    }
}
