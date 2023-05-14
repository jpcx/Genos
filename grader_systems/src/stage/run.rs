use std::{path::Path, time::Duration};

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use genos::{
    gs::running_in_gs,
    output::{self, Output, RichTextMaker, Section, StatusUpdates, Update},
    points::PointQuantity,
    process::{
        self, is_program_in_path, Command, ExitStatus, ProcessExecutor, SignalType, StdinPipe,
    },
    stage::{StageResult, StageStatus},
    Executor,
};
use tracing::debug;

// give a default timeout of 1 minute. Number chosen arbitrarily.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Default, Clone)]
pub struct RunConfig {
    pub args: Vec<String>,
    pub executable: String,
    pub timeout: Option<Duration>,
    pub stdout: Option<String>,
    pub stderr: Option<String>,
    pub stdin: Option<String>,
    pub return_code: Option<ReturnCodeConfig>,
    pub disable_garbage_memory: Option<bool>,
}

#[derive(Clone)]
pub struct ReturnCodeConfig {
    pub expected: i32,
    pub points: PointQuantity,
}

pub struct Run<E> {
    executor: E,
    config: RunConfig,
}

impl<E> Run<E>
where
    E: ProcessExecutor,
{
    pub fn new(executor: E, config: RunConfig) -> Self {
        // idk if this assert belongs here, but it should definitely go somewhere.
        if running_in_gs() {
            assert!(
                is_program_in_path("valgrind"),
                "expected valgrind to exist when in gs"
            );
        }
        Self { executor, config }
    }

    fn get_run_command(&self, ws: &Path) -> Command {
        let mut cmd = if !self.config.disable_garbage_memory.unwrap_or(false)
            && is_program_in_path("valgrind")
        {
            Command::new("valgrind")
                .arg("--log-file=valgrind.log")
                .arg("--malloc-fill=0xFF")
                .arg("--free-fill=0xAA")
                .arg(&self.config.executable)
                .args(&self.config.args)
        } else {
            Command::new(&self.config.executable).args(&self.config.args)
        };

        if let Some(stdout_file) = &self.config.stdout {
            cmd.set_stdout(stdout_file);
        }

        if let Some(stderr_file) = &self.config.stderr {
            cmd.set_stderr(stderr_file);
        }

        if let Some(stdin_file) = &self.config.stdin {
            cmd.set_stdin(StdinPipe::Path(stdin_file.into()));
        }

        cmd.set_timeout(self.config.timeout.unwrap_or(DEFAULT_TIMEOUT));
        cmd.set_cwd(ws);

        cmd
    }

    fn get_failed_run_notes(&self, res: &process::Output) -> output::Content {
        match &res.status {
            ExitStatus::Timeout(duration) => {
                format!("Runtime error: program timed out after {:?}", duration).into()
            }
            ExitStatus::Signal(signal) => get_signal_feedback(signal),
            _ => unreachable!(),
        }
    }
}

fn get_signal_feedback(signal: &SignalType) -> output::Content {
    match signal {
        SignalType::Abort => {
            "Runtime error: Your submission exited with error code 6 (abort signal)".into()
        }
        SignalType::SegFault => {
            let output =
                "Runtime error: Your submission exited with error code 11 (segmentation fault)
             Double check you initialized all your variables before using them.
             Check your variables again.
             Check any array access points to make sure you are in bounds.
             Check pointer dereferences, you may be accidentally dereferencing a NULL pointer.";

            output
                .split("\n")
                .map(|line| line.trim().to_string())
                .collect::<Vec<_>>()
                .join("\n")
                .into()
        }
    }
}

#[async_trait]
impl<E> Executor for Run<E>
where
    E: ProcessExecutor,
{
    type Output = StageResult;

    async fn run(&self, ws: &Path) -> Result<Self::Output> {
        let mut section = Section::new("Run Program");
        let mut run_status_updates = StatusUpdates::default();
        let mut points_lost = PointQuantity::zero();
        debug!("running program");

        let executable = ws.join(&self.config.executable);
        if !executable.exists() {
            return Err(anyhow!(
                "Could not find student executable at {:?}",
                executable.display()
            ));
        }

        let cmd = self.get_run_command(ws);
        section.add_content(("run command", format!("{}", cmd).code()));

        let res = cmd.run_with(&self.executor).await?;

        if !res.status.completed() {
            run_status_updates.add_update(
                Update::new_fail("Running program", PointQuantity::FullPoints)
                    .notes(self.get_failed_run_notes(&res)),
            );
            section.add_content(run_status_updates);
            return Ok(StageResult::new(
                StageStatus::UnrecoverableFailure,
                Some(Output::new().section(section)),
            ));
        }

        run_status_updates.add_update(Update::new_pass("Running program"));

        if let Some(rc_config) = &self.config.return_code {
            let rc = res
                .status
                .exit_code()
                .expect("Expected function to get a status from a command which completed");

            if rc != rc_config.expected {
                run_status_updates.add_update(
                    Update::new_fail("Checking return code", PointQuantity::FullPoints)
                        .notes(format!("Expected {}, but found {}", rc_config.expected, rc)),
                );
                section.add_content(run_status_updates);
                points_lost += rc_config.points;
                return Ok(StageResult::new_continue(points_lost)
                    .with_output(output::Output::new().section(section)));
            }

            run_status_updates.add_update(Update::new_pass("Checking return code"));
            section.add_content(run_status_updates);
        }

        Ok(StageResult::new_continue(points_lost)
            .with_output(output::Output::new().section(section)))
    }
}

#[cfg(test)]
mod tests {
    use std::env;

    use genos::{
        fs::filepath,
        output::Contains,
        test_util::{MockDir, MockProcessExecutor},
    };

    use super::*;

    #[tokio::test]
    async fn executable_does_not_exist() {
        let config = RunConfig {
            executable: "bin/exec".to_string(),
            ..Default::default()
        };
        let ws = tempfile::tempdir().unwrap();
        let executor = MockProcessExecutor::with_responses([]);

        Run::new(executor, config).run(ws.path()).await.unwrap_err();
    }

    #[tokio::test]
    async fn executor_timeout() {
        let config = RunConfig {
            executable: "exec".to_string(),
            ..Default::default()
        };
        let ws = MockDir::new().file(("exec", "content"));
        let executor = MockProcessExecutor::with_responses([Ok(
            process::Output::from_exit_status(ExitStatus::Timeout(Duration::from_millis(10))),
        )]);

        let res = Run::new(executor, config)
            .run(ws.root.path())
            .await
            .unwrap();

        assert_eq!(res.status, StageStatus::UnrecoverableFailure);
        assert!(res.output.unwrap().contains("program timed out"));
    }

    #[tokio::test]
    async fn executor_abort() {
        let config = RunConfig {
            executable: "exec".to_string(),
            ..Default::default()
        };
        let ws = MockDir::new().file(("exec", "content"));
        let executor = MockProcessExecutor::with_responses([Ok(
            process::Output::from_exit_status(ExitStatus::Signal(SignalType::Abort)),
        )]);

        let res = Run::new(executor, config)
            .run(ws.root.path())
            .await
            .unwrap();

        assert_eq!(res.status, StageStatus::UnrecoverableFailure);
        assert!(res.output.unwrap().contains("abort signal"));
    }

    #[tokio::test]
    async fn executor_segfault() {
        let config = RunConfig {
            executable: "exec".to_string(),
            ..Default::default()
        };
        let ws = MockDir::new().file(("exec", "content"));
        let executor = MockProcessExecutor::with_responses([Ok(
            process::Output::from_exit_status(ExitStatus::Signal(SignalType::SegFault)),
        )]);

        let res = Run::new(executor, config)
            .run(ws.root.path())
            .await
            .unwrap();

        assert_eq!(res.status, StageStatus::UnrecoverableFailure);
        assert!(res.output.unwrap().contains("segmentation fault"));
    }

    #[tokio::test]
    async fn success_no_return_code() {
        let config = RunConfig {
            executable: "exec".to_string(),
            ..Default::default()
        };
        let ws = MockDir::new().file(("exec", "content"));
        let executor = MockProcessExecutor::with_responses([Ok(
            process::Output::from_exit_status(ExitStatus::Failure(1)),
        )]);

        let res = Run::new(executor, config)
            .run(ws.root.path())
            .await
            .unwrap();

        assert_eq!(
            res.status,
            StageStatus::Continue {
                points_lost: PointQuantity::zero()
            }
        );
    }

    #[tokio::test]
    async fn wrong_return_code() {
        let config = RunConfig {
            executable: "exec".to_string(),
            return_code: Some(ReturnCodeConfig {
                expected: 0,
                points: PointQuantity::FullPoints,
            }),
            ..Default::default()
        };
        let ws = MockDir::new().file(("exec", "content"));
        let executor = MockProcessExecutor::with_responses([Ok(
            process::Output::from_exit_status(ExitStatus::Failure(1)),
        )]);

        let res = Run::new(executor, config)
            .run(ws.root.path())
            .await
            .unwrap();

        assert_eq!(
            res.status,
            StageStatus::Continue {
                points_lost: PointQuantity::FullPoints,
            }
        );
    }

    #[tokio::test]
    async fn correct_return_code() {
        let config = RunConfig {
            executable: "exec".to_string(),
            return_code: Some(ReturnCodeConfig {
                expected: 1,
                points: PointQuantity::FullPoints,
            }),
            ..Default::default()
        };
        let ws = MockDir::new().file(("exec", "content"));
        let executor = MockProcessExecutor::with_responses([Ok(
            process::Output::from_exit_status(ExitStatus::Failure(1)),
        )]);

        let res = Run::new(executor, config)
            .run(ws.root.path())
            .await
            .unwrap();

        assert_eq!(
            res.status,
            StageStatus::Continue {
                points_lost: PointQuantity::zero(),
            }
        );
    }

    #[test]
    fn get_run_command_default() {
        let config = RunConfig {
            executable: "bin/exec".to_string(),
            ..Default::default()
        };
        let ws = tempfile::tempdir().unwrap();
        let executor = MockProcessExecutor::with_responses([]);

        let run = Run::new(executor, config);

        let cmd = run.get_run_command(ws.path());
        let expected = Command::new("bin/exec");
        assert_eq!(cmd.to_string(), expected.to_string());
    }

    #[test]
    fn get_run_command_pipes() {
        let config = RunConfig {
            executable: "bin/exec".to_string(),
            stderr: Some("stderr".to_string()),
            stdin: Some("stdin".to_string()),
            stdout: Some("stdout".to_string()),
            ..Default::default()
        };
        let ws = tempfile::tempdir().unwrap();
        let executor = MockProcessExecutor::with_responses([]);

        let run = Run::new(executor, config);

        let cmd = run.get_run_command(ws.path());
        let expected = Command::new("bin/exec")
            .stdout("stdout")
            .stdin(StdinPipe::Path("stdin".into()))
            .stderr("stderr");
        assert_eq!(cmd.to_string(), expected.to_string());
    }

    #[test]
    fn get_run_command_valgrind() {
        let mock_dir = MockDir::new().file(("valgrind", ""));

        // fake that we have valgrind in our path
        let valgrind_path = mock_dir.root.path();
        let mut path = env::var("PATH").unwrap();
        path += format!(":{}", filepath(valgrind_path).unwrap()).as_str();
        env::set_var("PATH", path);

        let config = RunConfig {
            executable: "bin/exec".to_string(),
            args: vec!["-t".to_string(), "-u".to_string()],
            ..Default::default()
        };
        let ws = tempfile::tempdir().unwrap();
        let executor = MockProcessExecutor::with_responses([]);

        let run = Run::new(executor, config);

        let cmd = run.get_run_command(ws.path());
        let expected = Command::new("valgrind")
            .arg("--log-file=valgrind.log")
            .arg("--malloc-fill=0xFF")
            .arg("--free-fill=0xAA")
            .arg("bin/exec")
            .arg("-t")
            .arg("-u");

        assert_eq!(cmd.to_string(), expected.to_string());
    }
}
