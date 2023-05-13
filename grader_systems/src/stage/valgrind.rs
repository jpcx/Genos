use std::{path::Path, time::Duration};

use anyhow::{anyhow, Result};
use async_trait::async_trait;

use genos::{
    output::{self, Output, RichTextMaker, Section, StatusUpdates, Update},
    points::PointQuantity,
    process::{self, Command, ExitStatus, ProcessExecutor, SignalType, StdinPipe},
    stage::{StageResult, StageStatus},
    Executor,
};

use regex::{Captures, Regex};

use serde::Deserialize;
use tracing::debug;

// default should be longer than run stage default
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Default, Deserialize, Clone)]
pub struct ValgrindConfig {
    leak_check: Option<bool>,
    error_exitcode: Option<i32>,
    log_file: Option<String>,
    malloc_fill: Option<u8>,
    free_fill: Option<u8>,
    suppressions: Option<String>,
}

pub struct Valgrind<E> {
    executor: E,
    config: ValgrindConfig,
    executable: String,
    args: Vec<String>,
    stdin: Option<String>,
    timeout: Option<Duration>,
}

impl<E: ProcessExecutor> Valgrind<E> {
    pub fn new(
        executor: E,
        config: ValgrindConfig,
        executable: String,
        args: Vec<String>,
        stdin: Option<String>,
        timeout: Option<Duration>,
    ) -> Self {
        Self {
            executor,
            config,
            executable,
            args,
            stdin,
            timeout,
        }
    }

    fn get_run_command(&self, ws: &Path) -> Command {
        let mut cmd = Command::new("valgrind");

        if let Some(v) = &self.config.leak_check {
            cmd = cmd.arg(format!("--leak-check={:?}", v));
        }

        if let Some(v) = self.config.error_exitcode {
            cmd = cmd.arg(format!("--error-exitcode={}", v));
        }

        if let Some(v) = &self.config.log_file {
            cmd = cmd.arg(format!("--log-file={}", v));
        }

        if let Some(v) = self.config.malloc_fill {
            cmd = cmd.arg(format!("--malloc-fill=0x{:02X}", v));
        }

        if let Some(v) = self.config.free_fill {
            cmd = cmd.arg(format!("--free-fill=0x{:02X}", v));
        }

        if let Some(v) = &self.config.suppressions {
            cmd = cmd.arg(format!("--suppressions={}", v));
        }

        cmd = cmd.arg("--");
        cmd = cmd.arg(&self.executable);
        cmd = cmd.args(&self.args);
        if let Some(v) = &self.stdin {
            cmd.set_stdin(StdinPipe::Path(v.into()))
        }

        cmd.set_timeout(self.timeout.unwrap_or(DEFAULT_TIMEOUT));
        cmd.set_cwd(ws);

        cmd
    }
}

fn hide_paths(input: &str) -> String {
    Regex::new(r"/[^/\s]+")
        .unwrap()
        .replace_all(input, "<path hidden>")
        .to_string()
}

#[async_trait]
impl<E: ProcessExecutor> Executor for Valgrind<E> {
    type Output = StageResult;

    async fn run(&self, ws: &Path) -> Result<Self::Output> {
        let mut section = Section::new("Valgrind");
        let mut run_status_updates = StatusUpdates::default();
        debug!("running program");

        let executable = ws.join(&self.executable);
        if !executable.exists() {
            return Err(anyhow!(
                "Could not find student executable at {:?}",
                executable.display()
            ));
        }

        if let Some(v) = &self.config.suppressions {
            if !ws.join(&v).exists() {
                return Err(anyhow!("Could not find valgrind suppressions at {}", v));
            }
        }

        let cmd = self.get_run_command(ws);
        section.add_content(("run command", format!("{}", cmd).code()));

        let res = cmd.run_with(&self.executor).await?;

        if !res.status.completed() {
            run_status_updates.add_update(
                Update::new("Running valgrind")
                    .status(output::Status::Fail)
                    .points_lost(PointQuantity::FullPoints),
            );
            section.add_content(run_status_updates);
            return Ok(StageResult::new(
                StageStatus::UnrecoverableFailure,
                Some(Output::new().section(section)),
            ));
        }

        run_status_updates.add_update(Update::new("Running valgrind").status(output::Status::Pass));
        run_status_updates
            .add_update(Update::new("valgrind output:").notes(hide_paths(&res.stderr)));
        section.add_content(run_status_updates);

        Ok(StageResult::new(
            StageStatus::Continue {
                points_lost: PointQuantity::zero(),
            },
            Some(Output::new().section(section)),
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use genos::{
        output::Contains,
        test_util::{MockDir, MockExecutorInner, MockProcessExecutor},
    };

    use super::*;

    #[tokio::test]
    async fn cmd_default() {
        let data = Arc::new(Mutex::new(MockExecutorInner::with_responses([Ok(
            process::Output::from_exit_status(ExitStatus::Ok),
        )])));

        let executor = MockProcessExecutor::new(data.clone());

        let default = Valgrind::new(
            executor,
            ValgrindConfig::default(),
            "foo".to_string(),
            Vec::new(),
            None,
            None,
        );

        let ws = tempfile::tempdir().unwrap();

        let cmd = default.get_run_command(ws.path());

        assert!(cmd.to_string() == "valgrind -- foo");
    }

    #[tokio::test]
    async fn cmd_partial() {
        let data = Arc::new(Mutex::new(MockExecutorInner::with_responses([Ok(
            process::Output::from_exit_status(ExitStatus::Ok),
        )])));

        let executor = MockProcessExecutor::new(data.clone());

        let mut config = ValgrindConfig::default();
        config.error_exitcode = Some(200);
        config.free_fill = Some(0xDE);

        let default = Valgrind::new(executor, config, "foo".to_string(), Vec::new(), None, None);
        let ws = tempfile::tempdir().unwrap();
        let cmd = default.get_run_command(ws.path());

        assert!(cmd.to_string() == "valgrind --error-exitcode=200 --free-fill=0xDE -- foo");
    }

    #[tokio::test]
    async fn cmd_redir() {
        let data = Arc::new(Mutex::new(MockExecutorInner::with_responses([Ok(
            process::Output::from_exit_status(ExitStatus::Ok),
        )])));

        let executor = MockProcessExecutor::new(data.clone());

        let mut config = ValgrindConfig::default();
        config.error_exitcode = Some(200);
        config.free_fill = Some(0xDE);

        let default = Valgrind::new(
            executor,
            config,
            "foo".to_string(),
            Vec::new(),
            Some("bar".to_string()),
            None,
        );

        let ws = tempfile::tempdir().unwrap();

        let cmd = default.get_run_command(ws.path());

        assert!(cmd.to_string() == "valgrind --error-exitcode=200 --free-fill=0xDE -- foo < bar");
    }

    #[tokio::test]
    async fn missing_exec_fail() {
        let data = Arc::new(Mutex::new(MockExecutorInner::with_responses([Ok(
            process::Output::from_exit_status(ExitStatus::Ok),
        )])));

        let executor = MockProcessExecutor::new(data.clone());

        let default = Valgrind::new(
            executor,
            ValgrindConfig::default(),
            "noop".to_string(),
            Vec::new(),
            None,
            None,
        );

        let ws = MockDir::new();

        assert!(default.run(ws.root.path()).await.is_err());
    }

    #[tokio::test]
    async fn avail_exec_pass() {
        let data = Arc::new(Mutex::new(MockExecutorInner::with_responses([Ok(
            process::Output::from_exit_status(ExitStatus::Ok),
        )])));

        let executor = MockProcessExecutor::new(data.clone());

        let default = Valgrind::new(
            executor,
            ValgrindConfig::default(),
            "noop".to_string(),
            Vec::new(),
            None,
            None,
        );

        let ws = MockDir::new().file(("noop", "#!/bin/bash"));

        assert!(!default.run(&ws.root.path()).await.is_err());
    }

    #[tokio::test]
    async fn simple_pass() {
        let noop_err = "\
            ==55== Memcheck, a memory error detector\n\
            ==55== Copyright (C) 2002-2017, and GNU GPL'd, by Julian Seward et al.\n\
            ==55== Using Valgrind-3.15.0 and LibVEX; rerun with -h for copyright info\n\
            ==55== Command: ./noop\n\
            ==55== \n\
            ==55== \n\
            ==55== HEAP SUMMARY:\n\
            ==55==     in use at exit: 0 bytes in 0 blocks\n\
            ==55==   total heap usage: 0 allocs, 0 frees, 0 bytes allocated\n\
            ==55== \n\
            ==55== All heap blocks were freed -- no leaks are possible\n\
            ==55== \n\
            ==55== For lists of detected and suppressed errors, rerun with: -s\n\
            ==55== ERROR SUMMARY: 0 errors from 0 contexts (suppressed: 0 from 0)";

        let data = Arc::new(Mutex::new(MockExecutorInner::with_responses([Ok(
            process::Output::new(ExitStatus::Ok, "", noop_err),
        )])));

        let executor = MockProcessExecutor::new(data.clone());

        let default = Valgrind::new(
            executor,
            ValgrindConfig::default(),
            "./noop".to_string(),
            Vec::new(),
            None,
            None,
        );

        let ws = MockDir::new().file(("noop", ""));
        let res = default.run(ws.root.path()).await.unwrap();

        assert_eq!(
            res.status,
            StageStatus::Continue {
                points_lost: PointQuantity::zero(),
            }
        );
        assert!(res.output.as_ref().unwrap().contains("Valgrind"));
        assert!(res.output.as_ref().unwrap().contains("run command"));
        assert!(res.output.as_ref().unwrap().contains("valgrind -- ./noop"));
        assert!(res
            .output
            .as_ref()
            .unwrap()
            .contains("All heap blocks were freed -- no leaks are possible"));
    }

    #[tokio::test]
    async fn hides_absolute() {
        let pwd_err = "\
            ==22856== Memcheck, a memory error detector\n\
            ==22856== Copyright (C) 2002-2022, and GNU GPL'd, by Julian Seward et al.\n\
            ==22856== Using Valgrind-3.20.0 and LibVEX; rerun with -h for copyright info\n\
            ==22856== Command: ./pwd\n\
            ==22856== \n\
            /home/user/git/Genos\n\
            ==22856== \n\
            ==22856== HEAP SUMMARY:\n\
            ==22856==     in use at exit: 0 bytes in 0 blocks\n\
            ==22856==   total heap usage: 1 allocs, 1 frees, 1,024 bytes allocated\n\
            ==22856== \n\
            ==22856== All heap blocks were freed -- no leaks are possible\n\
            ==22856== \n\
            ==22856== For lists of detected and suppressed errors, rerun with: -s\n\
            ==22856== ERROR SUMMARY: 0 errors from 0 contexts (suppressed: 0 from 0";

        let data = Arc::new(Mutex::new(MockExecutorInner::with_responses([Ok(
            process::Output::new(ExitStatus::Ok, "", pwd_err),
        )])));

        let executor = MockProcessExecutor::new(data.clone());

        let default = Valgrind::new(
            executor,
            ValgrindConfig::default(),
            "./pwd".to_string(),
            Vec::new(),
            None,
            None,
        );

        let ws = MockDir::new().file(("pwd", ""));
        let res = default.run(ws.root.path()).await.unwrap();

        assert_eq!(
            res.status,
            StageStatus::Continue {
                points_lost: PointQuantity::zero(),
            }
        );
        assert!(res.output.as_ref().unwrap().contains("Valgrind"));
        assert!(res.output.as_ref().unwrap().contains("run command"));
        assert!(res.output.as_ref().unwrap().contains("valgrind -- ./pwd"));
        assert!(res
            .output
            .as_ref()
            .unwrap()
            .contains("All heap blocks were freed -- no leaks are possible"));
        assert!(!res
            .output
            .as_ref()
            .unwrap()
            .contains("/home/user/git/Genos"));
        assert!(res.output.as_ref().unwrap().contains("<path hidden>"));
    }
}
