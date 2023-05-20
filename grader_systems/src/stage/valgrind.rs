use std::{
    fs::File,
    io::{self, Read},
    path::Path,
    time::Duration,
};

use anyhow::{anyhow, Result};
use async_trait::async_trait;

use genos::{
    output::{self, Content, Output, RichTextMaker, Section, StatusUpdates, Update},
    points::PointQuantity,
    process::{self, Command, ExitStatus, ProcessExecutor, SignalType, StdinPipe},
    stage::{StageResult, StageStatus},
    Executor,
};

use regex::{Captures, Regex};

use serde::Deserialize;

use tokio::sync::OnceCell;

use tracing::debug;

// default should be longer than run stage default
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);

// exit code used to identify failures. not configurable via YAML.
//
// exit codes >125 are reserved by POSIX standards, see exit(1p).
// note: >128 indicates that the command was interrupted by a signal.
// see also: https://tldp.org/LDP/abs/html/exitcodes.html
//
// setting --error-exitcode to 125 allows a failure condition
// to be reliably detected if the exit code is >= this number.
// valgrind will return >128 if the program was interrupted
// by a signal, regardless of this setting (on POSIX systems).
//
// as a safeguard, "ERROR SUMMARY: [1-9]" also signifies failure.
const ERROR_EXITCODE: i16 = 125;

fn is_exit_error(code: i16) -> bool {
    return code >= ERROR_EXITCODE;
}

#[derive(Debug, Default, Deserialize, Clone)]
pub struct ValgrindConfig {
    log_file: String,
    leak_check: Option<bool>,
    malloc_fill: Option<u8>,
    free_fill: Option<u8>,
    suppressions: Option<String>,
}

impl ValgrindConfig {
    fn new(log_file: &str) -> Self {
        Self {
            log_file: log_file.to_string(),
            ..Self::default()
        }
    }
}

pub struct Valgrind<E> {
    executor: E,
    config: ValgrindConfig,
    executable: String,
    args: Vec<String>,
    stdin: Option<String>,
    timeout: Option<Duration>,
}

fn read_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;
    Ok(contents)
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

    fn gen_cmd(&self, ws: &Path) -> Command {
        let mut cmd = Command::new("valgrind");

        cmd.add_arg(format!("--log-file={}", &self.config.log_file));

        if let Some(v) = &self.config.leak_check {
            cmd.add_arg(format!("--leak-check={:?}", v));
        }

        cmd.add_arg(format!("--error-exitcode={}", ERROR_EXITCODE));

        if let Some(v) = self.config.malloc_fill {
            cmd.add_arg(format!("--malloc-fill=0x{:02X}", v));
        }

        if let Some(v) = self.config.free_fill {
            cmd.add_arg(format!("--free-fill=0x{:02X}", v));
        }

        if let Some(v) = &self.config.suppressions {
            cmd.add_arg(format!("--suppressions={}", v));
        }

        cmd.add_arg("--");
        cmd.add_arg(&self.executable);
        cmd.add_args(&self.args);
        if let Some(v) = &self.stdin {
            cmd.set_stdin(StdinPipe::Path(v.into()))
        }

        cmd.set_timeout(self.timeout.unwrap_or(DEFAULT_TIMEOUT));
        cmd.set_cwd(ws);

        cmd
    }

    fn read_logfile(&self, ws: &Path) -> Result<String> {
        let contents = read_file(&ws.join(&self.config.log_file))?;
        // replace all absolute paths with basename
        let re = Regex::new(r"(\W|^)(?:\/[^\/\s]+)+\/([^\/\s]+)\b")?;
        let repl = re.replace_all(&contents, "$1$2");
        Ok(repl.to_string())
    }
}

#[async_trait]
impl<E: ProcessExecutor> Executor for Valgrind<E> {
    type Output = StageResult;

    async fn run(&self, ws: &Path) -> Result<Self::Output> {
        let mut sect = Section::new("Valgrind");
        let mut results_sect = Section::new("Output:");
        let mut run_updates = StatusUpdates::default();
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

        //let cmd = self.gen_cmd(ws);
        //sect.add_content(("run command", format!("{}", cmd).code()));

        //let res = cmd.run_with(&self.executor).await?;

        //if !res.status.completed() {
        //    run_updates.add_update(Update::new_fail(
        //        "Running valgrind",
        //        PointQuantity::FullPoints,
        //    ));
        //    sect.add_content(run_updates);
        //    return Ok(StageResult::new(
        //        StageStatus::UnrecoverableFailure,
        //        Some(Output::new().section(sect)),
        //    ));
        //}

        //run_updates.add_update(Update::new_pass("Running valgrind"));
        //results_sect.add_content(self.read_logfile()?);

        //sect.add_content(run_updates);
        //sect.add_content(Content::SubSection(results_sect));

        Ok(StageResult::new(
            StageStatus::Continue {
                points_lost: PointQuantity::zero(),
            },
            Some(Output::new().section(sect)),
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::{
        path::PathBuf,
        sync::{Arc, Mutex},
    };

    use genos::{
        output::Contains,
        test_util::{MockDir, MockExecutorInner, MockProcessExecutor},
    };

    use super::*;

    #[derive(Deserialize)]
    struct ExamplesPrePost {
        pre: String,
        post: String,
    }

    #[derive(Deserialize)]
    struct ExamplesDebugRelease<T> {
        debug: T,
        release: T,
    }

    #[derive(Deserialize)]
    struct Examples {
        noop: String,
        segfault: ExamplesPrePost,
        agony: ExamplesDebugRelease<ExamplesPrePost>,
    }

    const VALGRIND_EXAMPLES: &'static str = "resources/valgrind/examples.yml";

    static EXAMPLES: OnceCell<Examples> = OnceCell::const_new();

    async fn read_examples() -> Result<Examples> {
        let mut examples_in = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        examples_in.push(VALGRIND_EXAMPLES);
        assert!(
            examples_in.exists(),
            "Expected to find valgrind examples at {}",
            VALGRIND_EXAMPLES
        );

        Ok(serde_yaml::from_str::<Examples>(&read_file(examples_in.as_path()).unwrap()).unwrap())
    }

    async fn get_examples() -> Result<&'static Examples> {
        EXAMPLES.get_or_try_init(read_examples).await
    }

    #[tokio::test]
    async fn cmd_basic() {
        let data = Arc::new(Mutex::new(MockExecutorInner::with_responses([Ok(
            process::Output::from_exit_status(ExitStatus::Ok),
        )])));

        let executor = MockProcessExecutor::new(data.clone());

        let vg = Valgrind::new(
            executor,
            ValgrindConfig::new("valgrind.log"),
            "foo".to_string(),
            Vec::new(),
            None,
            None,
        );
        let ws = tempfile::tempdir().unwrap();
        let cmd = vg.gen_cmd(ws.path());

        assert_eq!(
            cmd.to_string(),
            format!(
                "valgrind --log-file=valgrind.log --error-exitcode={} -- foo",
                ERROR_EXITCODE
            )
        );
    }

    #[tokio::test]
    async fn cmd_options() {
        let data = Arc::new(Mutex::new(MockExecutorInner::with_responses([Ok(
            process::Output::from_exit_status(ExitStatus::Ok),
        )])));

        let executor = MockProcessExecutor::new(data.clone());

        let mut config = ValgrindConfig::new("valgrind.log");
        config.malloc_fill = Some(0xBA);
        config.free_fill = Some(0xDE);

        let vg = Valgrind::new(executor, config, "foo".to_string(), Vec::new(), None, None);
        let ws = tempfile::tempdir().unwrap();
        let cmd = vg.gen_cmd(ws.path());

        assert_eq!(
            cmd.to_string(),
            format!(
                "valgrind --log-file=valgrind.log --error-exitcode={} \
                --malloc-fill=0xBA --free-fill=0xDE -- foo",
                ERROR_EXITCODE
            )
        );
    }

    #[tokio::test]
    async fn cmd_redir_stdin() {
        let data = Arc::new(Mutex::new(MockExecutorInner::with_responses([Ok(
            process::Output::from_exit_status(ExitStatus::Ok),
        )])));

        let executor = MockProcessExecutor::new(data.clone());

        let mut config = ValgrindConfig::new("valgrind.log");
        config.malloc_fill = Some(0xBA);
        config.free_fill = Some(0xDE);

        let vg = Valgrind::new(
            executor,
            config,
            "foo".to_string(),
            Vec::new(),
            Some("bar".to_string()),
            None,
        );

        let ws = tempfile::tempdir().unwrap();

        let cmd = vg.gen_cmd(ws.path());

        assert_eq!(
            cmd.to_string(),
            format!(
                "valgrind --log-file=valgrind.log --error-exitcode={} \
                --malloc-fill=0xBA --free-fill=0xDE -- foo < bar",
                ERROR_EXITCODE
            )
        );
    }

    #[tokio::test]
    async fn read_any() {
        let data = Arc::new(Mutex::new(MockExecutorInner::with_responses([Ok(
            process::Output::from_exit_status(ExitStatus::Ok),
        )])));

        let executor = MockProcessExecutor::new(data.clone());

        let vg = Valgrind::new(
            executor,
            ValgrindConfig::new("valgrind.log"),
            "noop".to_string(),
            Vec::new(),
            None,
            None,
        );

        let ws = MockDir::new()
            .file(("noop", ""))
            .file(("valgrind.log", "here is some text"));

        assert_eq!(
            vg.read_logfile(ws.root.path()).unwrap(),
            "here is some text"
        );
    }

    #[tokio::test]
    async fn read_basic() {
        let data = Arc::new(Mutex::new(MockExecutorInner::with_responses([Ok(
            process::Output::from_exit_status(ExitStatus::Ok),
        )])));

        let executor = MockProcessExecutor::new(data.clone());

        let vg = Valgrind::new(
            executor,
            ValgrindConfig::new("valgrind.log"),
            "noop".to_string(),
            Vec::new(),
            None,
            None,
        );

        let log = get_examples().await.unwrap().noop.clone();

        assert!(log.find("ERROR SUMMARY").is_some());

        let ws = MockDir::new()
            .file(("noop", ""))
            .file(("valgrind.log", log.clone()));

        assert_eq!(vg.read_logfile(ws.root.path()).unwrap(), log);
    }

    #[tokio::test]
    async fn read_hides_paths_basic() {
        let data = Arc::new(Mutex::new(MockExecutorInner::with_responses([Ok(
            process::Output::from_exit_status(ExitStatus::Ok),
        )])));

        let executor = MockProcessExecutor::new(data.clone());

        let vg = Valgrind::new(
            executor,
            ValgrindConfig::new("valgrind.log"),
            "noop".to_string(),
            Vec::new(),
            None,
            None,
        );

        let log_await = &get_examples().await.unwrap().segfault;
        let log_pre = log_await.pre.clone();
        let log_post = log_await.post.clone();

        assert!(log_pre.find("ERROR SUMMARY").is_some());
        assert!(log_post.find("ERROR SUMMARY").is_some());

        assert!(log_pre.find("/root/genos_tests/segfault").is_some());
        assert!(log_post.find("/root/genos_tests/segfault").is_none());
        assert!(log_pre.find("(in segfault)").is_none());
        assert!(log_post.find("(in segfault)").is_some());

        let ws = MockDir::new()
            .file(("noop", ""))
            .file(("valgrind.log", log_pre));

        assert_eq!(vg.read_logfile(ws.root.path()).unwrap(), log_post);
    }

    #[tokio::test]
    async fn read_hides_paths_many_debug_mode() {
        let data = Arc::new(Mutex::new(MockExecutorInner::with_responses([Ok(
            process::Output::from_exit_status(ExitStatus::Ok),
        )])));

        let executor = MockProcessExecutor::new(data.clone());

        let vg = Valgrind::new(
            executor,
            ValgrindConfig::new("valgrind.log"),
            "noop".to_string(),
            Vec::new(),
            None,
            None,
        );

        let log_await = &get_examples().await.unwrap().agony.debug;
        let log_pre = log_await.pre.clone();
        let log_post = log_await.post.clone();

        assert!(log_pre.find("ERROR SUMMARY").is_some());
        assert!(log_post.find("ERROR SUMMARY").is_some());

        assert!(log_pre
            .find("/usr/lib/x86_64-linux-gnu/valgrind/vgpreload_memcheck-amd64-linux.so")
            .is_some());
        assert!(log_post
            .find("/usr/lib/x86_64-linux-gnu/valgrind/vgpreload_memcheck-amd64-linux.so")
            .is_none());
        assert!(log_pre
            .find("(in vgpreload_memcheck-amd64-linux.so)")
            .is_none());
        assert!(log_post
            .find("(in vgpreload_memcheck-amd64-linux.so)")
            .is_some());

        let ws = MockDir::new()
            .file(("noop", ""))
            .file(("valgrind.log", log_pre));

        assert_eq!(vg.read_logfile(ws.root.path()).unwrap(), log_post);
    }

    #[tokio::test]
    async fn read_hides_paths_many_release_mode() {
        let data = Arc::new(Mutex::new(MockExecutorInner::with_responses([Ok(
            process::Output::from_exit_status(ExitStatus::Ok),
        )])));

        let executor = MockProcessExecutor::new(data.clone());

        let vg = Valgrind::new(
            executor,
            ValgrindConfig::new("valgrind.log"),
            "noop".to_string(),
            Vec::new(),
            None,
            None,
        );

        let log_await = &get_examples().await.unwrap().agony.release;
        let log_pre = log_await.pre.clone();
        let log_post = log_await.post.clone();

        assert!(log_pre
            .find("/root/genos_tests/many/layers/of/agony")
            .is_some());
        assert!(log_post
            .find("/root/genos_tests/many/layers/of/agony")
            .is_none());
        assert!(log_pre
            .find("/usr/lib/x86_64-linux-gnu/valgrind/vgpreload_memcheck-amd64-linux.so")
            .is_some());
        assert!(log_post
            .find("/usr/lib/x86_64-linux-gnu/valgrind/vgpreload_memcheck-amd64-linux.so")
            .is_none());
        assert!(log_pre
            .find("(in vgpreload_memcheck-amd64-linux.so)")
            .is_none());
        assert!(log_post
            .find("(in vgpreload_memcheck-amd64-linux.so)")
            .is_some());
        assert!(log_pre.find("(in agony)").is_none());
        assert!(log_post.find("(in agony)").is_some());

        let ws = MockDir::new()
            .file(("noop", ""))
            .file(("valgrind.log", log_pre));

        assert_eq!(vg.read_logfile(ws.root.path()).unwrap(), log_post);
    }

    #[tokio::test]
    async fn run_binchk_pass() {
        let data = Arc::new(Mutex::new(MockExecutorInner::with_responses([Ok(
            process::Output::from_exit_status(ExitStatus::Ok),
        )])));

        let executor = MockProcessExecutor::new(data.clone());

        let vg = Valgrind::new(
            executor,
            ValgrindConfig::new("valgrind.log"),
            "noop".to_string(),
            Vec::new(),
            None,
            None,
        );

        let ws = MockDir::new().file(("noop", ""));

        assert!(!vg.run(ws.root.path()).await.is_err());
    }

    #[tokio::test]
    async fn run_binchk_fail() {
        let data = Arc::new(Mutex::new(MockExecutorInner::with_responses([Ok(
            process::Output::from_exit_status(ExitStatus::Ok),
        )])));

        let executor = MockProcessExecutor::new(data.clone());

        let vg = Valgrind::new(
            executor,
            ValgrindConfig::new("valgrind.log"),
            "noop".to_string(),
            Vec::new(),
            None,
            None,
        );

        let ws = MockDir::new();

        assert!(vg.run(ws.root.path()).await.is_err());
    }

    #[tokio::test]
    async fn run_suppchk_pass() {
        let data = Arc::new(Mutex::new(MockExecutorInner::with_responses([Ok(
            process::Output::from_exit_status(ExitStatus::Ok),
        )])));

        let executor = MockProcessExecutor::new(data.clone());

        let mut config = ValgrindConfig::new("valgrind.log");
        config.suppressions = Some("foo.supp".to_string());

        let vg = Valgrind::new(executor, config, "noop".to_string(), Vec::new(), None, None);
        let ws = MockDir::new().file(("noop", "")).file(("foo.supp", ""));

        assert!(!vg.run(ws.root.path()).await.is_err());
    }

    #[tokio::test]
    async fn run_suppchk_fail() {
        let data = Arc::new(Mutex::new(MockExecutorInner::with_responses([Ok(
            process::Output::from_exit_status(ExitStatus::Ok),
        )])));

        let executor = MockProcessExecutor::new(data.clone());

        let mut config = ValgrindConfig::new("valgrind.log");
        config.suppressions = Some("foo.supp".to_string());

        let vg = Valgrind::new(executor, config, "noop".to_string(), Vec::new(), None, None);
        let ws = MockDir::new().file(("noop", ""));

        assert!(vg.run(ws.root.path()).await.is_err());
    }
}
