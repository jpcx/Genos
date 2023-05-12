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
    score::Score,
    stage::{StageResult, StageStatus},
    Executor,
};
use tracing::debug;

// give a default timeout of 1 minute. Number chosen arbitrarily.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

pub struct RunConfig {
    args: Vec<String>,
    executable: String,
    timeout: Option<Duration>,
    stdout: Option<String>,
    stderr: Option<String>,
    stdin: Option<String>,
    return_code: Option<ReturnCodeConfig>,
    disable_garbage_memory: Option<bool>,
}

pub struct ReturnCodeConfig {
    expected: i32,
    points: PointQuantity,
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

    fn check_return_code(
        &self,
        rc_config: &ReturnCodeConfig,
        status: &ExitStatus,
    ) -> (Update, Score) {
        let rc = status.exit_code().expect(
            "Expected function to get a status from a command which completed successfully",
        );

        todo!()
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
                Update::new("Running program")
                    .status(output::Status::Fail)
                    .points_lost(PointQuantity::FullPoints)
                    .notes(self.get_failed_run_notes(&res)),
            );
            section.add_content(run_status_updates);
            return Ok(StageResult::new(
                StageStatus::UnrecoverableFailure,
                Some(Output::new().section(section)),
            ));
        }

        run_status_updates.add_update(Update::new("Running program").status(output::Status::Pass));

        if let Some(rc_config) = &self.config.return_code {
            let (update, score) = self.check_return_code(rc_config, &res.status);
        }

        todo!();
    }
}
