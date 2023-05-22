use std::{fs::File, io::Read, path::Path, time::Duration};

use anyhow::{anyhow, Result};
use async_trait::async_trait;

use genos::{
    output::{Content, Output, RichTextMaker, Section, StatusUpdates, Update},
    points::PointQuantity,
    process::{Command, ExitStatus, ProcessExecutor, SignalType, StdinPipe},
    stage::{StageResult, StageStatus},
    Executor,
};

use regex::Regex;

use serde::Deserialize;

use tracing::debug;

// default should be longer than run stage default
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);

// exit code used to identify non-signal failures.
// not configurable via YAML.
//
// exit codes >125 are reserved by POSIX; see exit(1p)
// and https://tldp.org/LDP/abs/html/exitcodes.html
const ERROR_EXITCODE: i32 = 125;

#[derive(Debug, Default, Deserialize, Clone)]
pub struct ValgrindConfig {
    log_file: String,
    leak_check: Option<bool>,
    malloc_fill: Option<u8>,
    free_fill: Option<u8>,
    suppressions: Option<Vec<String>>,
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
    #[allow(dead_code)]
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
            cmd.add_arg(format!("--leak-check={}", if !!v { "yes" } else { "no" }));
        }

        cmd.add_arg(format!("--error-exitcode={}", ERROR_EXITCODE));

        if let Some(v) = self.config.malloc_fill {
            cmd.add_arg(format!("--malloc-fill=0x{:02X}", v));
        }

        if let Some(v) = self.config.free_fill {
            cmd.add_arg(format!("--free-fill=0x{:02X}", v));
        }

        if let Some(v) = &self.config.suppressions {
            for supp in v {
                cmd.add_arg(format!("--suppressions={}", supp));
            }
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
        let path = ws.join(&self.config.log_file);
        if !path.exists() {
            return Err(anyhow!(
                "Could not find logfile at {:?}. Did Valgrind actually execute?",
                path.to_str()
            ));
        }
        let contents = read_file(&path)?;
        // replace all absolute paths with basename
        let re = Regex::new(r"(\W|^)(?:\/[^\/\s]+)+\/([^\/\s]+)\b")?;
        let repl = re.replace_all(&contents, "$1$2");

        if repl.trim().is_empty() {
            return Err(anyhow!(
                "Found empty valgrind log at {:?}. Something went wrong.",
                path.to_str()
            ));
        }

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
            for supp in v {
                if !ws.join(&supp).exists() {
                    return Err(anyhow!("Could not find valgrind suppressions at {}", supp));
                }
            }
        }

        let cmd = self.gen_cmd(ws);
        sect.add_content(("Run Command", format!("{}", cmd).code()));

        let res = cmd.run_with(&self.executor).await?;

        let log = self.read_logfile(ws)?;

        if !res.status.completed() {
            match res.status {
                ExitStatus::Timeout(to) => {
                    run_updates.add_update(
                        Update::new_fail("running valgrind", PointQuantity::FullPoints).notes(
                            format!(
                                "Your submission timed out after {} seconds :(",
                                to.as_secs()
                            ),
                        ),
                    );
                    sect.add_content(run_updates);
                }
                ExitStatus::Signal(sig) => match sig {
                    SignalType::SegFault => {
                        run_updates.add_update(
                            Update::new_fail("running valgrind", PointQuantity::FullPoints)
                                .notes("Your submission was killed by SIGSEGV!"),
                        );
                        results_sect.add_content(log);
                        sect.add_content(run_updates);
                        sect.add_content(Content::SubSection(results_sect));
                    }
                    SignalType::Abort => {
                        run_updates.add_update(
                            Update::new_fail("running valgrind", PointQuantity::FullPoints)
                                .notes("Your submission was killed by SIGABRT!"),
                        );
                        results_sect.add_content(log);
                        sect.add_content(run_updates);
                        sect.add_content(Content::SubSection(results_sect));
                    }
                },
                _ => panic!("Expected either Timeout or Signal if command was not completed"),
            }
            return Ok(StageResult::new(
                StageStatus::UnrecoverableFailure,
                Some(Output::new().section(sect)),
            ));
        }

        match res.status {
            ExitStatus::Ok => {
                run_updates.add_update(Update::new_pass("running valgrind"));
                results_sect.add_content(log);
                sect.add_content(run_updates);
                sect.add_content(Content::SubSection(results_sect));

                Ok(StageResult::new(
                    StageStatus::Continue {
                        points_lost: PointQuantity::zero(),
                    },
                    Some(Output::new().section(sect)),
                ))
            }
            ExitStatus::Failure(rc) => {
                if rc < ERROR_EXITCODE {
                    return Err(anyhow!(
                        "Error: expected exit code to be at least {}, was {}",
                        ERROR_EXITCODE,
                        rc
                    ));
                }

                run_updates.add_update(
                    Update::new_fail("running valgrind", PointQuantity::FullPoints)
                        .notes("Valgrind Errors Detected"),
                );
                results_sect.add_content(log);
                sect.add_content(run_updates);
                sect.add_content(Content::SubSection(results_sect));

                return Ok(StageResult::new(
                    StageStatus::UnrecoverableFailure,
                    Some(Output::new().section(sect)),
                ));
            }
            _ => panic!("Expected either Ok or Failure ExitStatus if the command completed"),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        path::PathBuf,
        sync::{Arc, Mutex},
    };

    use genos::{
        formatter::MarkdownFormatter,
        output::Contains,
        process,
        test_util::{MockDir, MockExecutorInner, MockProcessExecutor},
        writer::Transform,
    };

    use super::*;

    use tokio::sync::OnceCell;

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
        infinite: ExamplesPrePost,
        segfault: ExamplesPrePost,
        agony: ExamplesDebugRelease<ExamplesPrePost>,
    }

    impl ValgrindConfig {
        fn new(log_file: &str) -> Self {
            Self {
                log_file: log_file.to_string(),
                ..Self::default()
            }
        }
    }

    // reads various valgrind output examples from YAML
    // use get_examples to access
    const EXAMPLES_PATH: &'static str = "resources/valgrind/examples.yml";
    static EXAMPLES: OnceCell<Examples> = OnceCell::const_new();

    async fn read_examples() -> Result<Examples> {
        let mut examples_in = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        examples_in.push(EXAMPLES_PATH);
        assert!(
            examples_in.exists(),
            "Expected to find valgrind examples at {}",
            EXAMPLES_PATH
        );

        Ok(serde_yaml::from_str::<Examples>(&read_file(examples_in.as_path()).unwrap()).unwrap())
    }

    // access loaded examples
    async fn get_examples() -> Result<&'static Examples> {
        EXAMPLES.get_or_try_init(read_examples).await
    }

    fn mock_valgrind(
        config: ValgrindConfig,
        exec: &str,
        args: Vec<String>,
        stdin: Option<String>,
        timeout: Option<Duration>,
        estatus: ExitStatus,
    ) -> Valgrind<MockProcessExecutor> {
        Valgrind::new(
            MockProcessExecutor::new(Arc::new(Mutex::new(MockExecutorInner::with_responses([
                Ok(process::Output::from_exit_status(estatus)),
            ])))),
            config,
            exec.to_string(),
            args,
            stdin,
            timeout,
        )
    }

    fn mock_cmd(
        config: ValgrindConfig,
        exec: &str,
        stdin: Option<String>,
        estatus: ExitStatus,
        ws: MockDir,
    ) -> String {
        mock_valgrind(config, exec, Vec::new(), stdin, None, estatus)
            .gen_cmd(ws.root.path())
            .to_string()
    }

    #[tokio::test]
    async fn cmd_reflects_config() {
        {
            assert_eq!(
                mock_cmd(
                    ValgrindConfig::new("vg.log"),
                    "noop",
                    None,
                    ExitStatus::Ok,
                    MockDir::new(),
                ),
                format!(
                    "valgrind --log-file=vg.log --error-exitcode={} -- noop",
                    ERROR_EXITCODE
                )
            );
        }

        {
            let mut config = ValgrindConfig::new("vg.log");
            config.malloc_fill = Some(0xBA);
            config.free_fill = Some(0xDE);
            assert_eq!(
                mock_cmd(config, "noop", None, ExitStatus::Ok, MockDir::new()),
                format!(
                    "valgrind --log-file=vg.log --error-exitcode={} \
                     --malloc-fill=0xBA --free-fill=0xDE -- noop",
                    ERROR_EXITCODE
                )
            );
        }

        {
            let mut config = ValgrindConfig::new("vg.log");
            config.malloc_fill = Some(0xBA);
            config.free_fill = Some(0xDE);
            config.leak_check = Some(true);
            assert_eq!(
                mock_cmd(config, "noop", None, ExitStatus::Ok, MockDir::new()),
                format!(
                    "valgrind --log-file=vg.log --leak-check=yes \
                     --error-exitcode={} --malloc-fill=0xBA \
                     --free-fill=0xDE -- noop",
                    ERROR_EXITCODE
                )
            );
        }

        {
            let mut config = ValgrindConfig::new("vg.log");
            config.malloc_fill = Some(0xBA);
            config.free_fill = Some(0xDE);
            config.leak_check = Some(false);
            assert_eq!(
                mock_cmd(config, "noop", None, ExitStatus::Ok, MockDir::new()),
                format!(
                    "valgrind --log-file=vg.log --leak-check=no \
                     --error-exitcode={} --malloc-fill=0xBA \
                     --free-fill=0xDE -- noop",
                    ERROR_EXITCODE
                )
            );
        }

        {
            let mut config = ValgrindConfig::new("vg.log");
            config.malloc_fill = Some(0xBA);
            config.free_fill = Some(0xDE);
            assert_eq!(
                mock_cmd(
                    config,
                    "noop",
                    Some("bar".to_string()),
                    ExitStatus::Ok,
                    MockDir::new()
                ),
                format!(
                    "valgrind --log-file=vg.log --error-exitcode={} \
                     --malloc-fill=0xBA --free-fill=0xDE -- noop < bar",
                    ERROR_EXITCODE
                )
            );
        }

        {
            let mut config = ValgrindConfig::new("vg.log");
            config.malloc_fill = Some(0xBA);
            config.free_fill = Some(0xDE);
            config.suppressions = Some(vec!["foo.supp".to_string()]);
            assert_eq!(
                mock_cmd(
                    config,
                    "noop",
                    Some("bar".to_string()),
                    ExitStatus::Ok,
                    MockDir::new()
                ),
                format!(
                    "valgrind --log-file=vg.log --error-exitcode={} \
                     --malloc-fill=0xBA --free-fill=0xDE --suppressions=foo.supp \
                     -- noop < bar",
                    ERROR_EXITCODE
                )
            );
        }

        {
            let mut config = ValgrindConfig::new("vg.log");
            config.malloc_fill = Some(0xBA);
            config.free_fill = Some(0xDE);
            config.suppressions = Some(vec!["foo.supp".to_string(), "bar.supp".to_string()]);
            assert_eq!(
                mock_cmd(
                    config,
                    "noop",
                    Some("bar".to_string()),
                    ExitStatus::Ok,
                    MockDir::new()
                ),
                format!(
                    "valgrind --log-file=vg.log --error-exitcode={} \
                     --malloc-fill=0xBA --free-fill=0xDE --suppressions=foo.supp \
                     --suppressions=bar.supp -- noop < bar",
                    ERROR_EXITCODE
                )
            );
        }
    }

    fn mock_read(config: ValgrindConfig, exec: &str, ws: MockDir) -> Result<String> {
        mock_valgrind(config, exec, Vec::new(), None, None, ExitStatus::Ok)
            .read_logfile(ws.root.path())
    }

    #[tokio::test]
    async fn read_validates_logfile() {
        assert!(mock_read(
            ValgrindConfig::new("vg.log"),
            "noop",
            MockDir::new().file(("noop", ""))
        )
        .is_err());

        assert!(mock_read(
            ValgrindConfig::new("vg.log"),
            "noop",
            MockDir::new().file(("noop", "")).file(("vg.log", ""))
        )
        .is_err());

        assert!(mock_read(
            ValgrindConfig::new("vg.log"),
            "noop",
            MockDir::new().file(("noop", "")).file(("vg.log", "\n\n"))
        )
        .is_err());

        assert!(mock_read(
            ValgrindConfig::new("vg.log"),
            "noop",
            MockDir::new()
                .file(("noop", ""))
                .file(("vg.log", get_examples().await.unwrap().noop.clone()))
        )
        .is_ok());

        assert!(mock_read(
            ValgrindConfig::new("vg.log"),
            "noop",
            MockDir::new()
                .file(("infinite", ""))
                .file(("vg.log", get_examples().await.unwrap().infinite.pre.clone()))
        )
        .is_ok());
    }

    #[tokio::test]
    async fn read_hides_absolute_paths() {
        let noop = &get_examples().await.unwrap().noop;
        let infinite = &get_examples().await.unwrap().infinite;
        let segfault = &get_examples().await.unwrap().segfault;
        let agony = &get_examples().await.unwrap().agony;

        assert_eq!(
            mock_read(
                ValgrindConfig::new("vg.log"),
                "noop",
                MockDir::new()
                    .file(("noop", ""))
                    .file(("vg.log", noop.clone()))
            )
            .unwrap(),
            noop.clone()
        );

        assert_eq!(
            mock_read(
                ValgrindConfig::new("vg.log"),
                "infinite",
                MockDir::new()
                    .file(("infinite", ""))
                    .file(("vg.log", infinite.pre.clone()))
            )
            .unwrap(),
            infinite.post.clone()
        );

        assert_eq!(
            mock_read(
                ValgrindConfig::new("vg.log"),
                "noop",
                MockDir::new()
                    .file(("noop", ""))
                    .file(("vg.log", segfault.pre.clone()))
            )
            .unwrap(),
            segfault.post.clone()
        );

        assert_eq!(
            mock_read(
                ValgrindConfig::new("vg.log"),
                "noop",
                MockDir::new()
                    .file(("noop", ""))
                    .file(("vg.log", agony.debug.pre.clone()))
            )
            .unwrap(),
            agony.debug.post.clone()
        );

        assert_eq!(
            mock_read(
                ValgrindConfig::new("vg.log"),
                "noop",
                MockDir::new()
                    .file(("noop", ""))
                    .file(("vg.log", agony.release.pre.clone()))
            )
            .unwrap(),
            agony.release.post.clone()
        );

        // sanity checks {{{
        assert!(noop.find("ERROR SUMMARY").is_some());
        assert!(infinite.pre.find("ERROR SUMMARY").is_some());
        assert!(infinite.post.find("ERROR SUMMARY").is_some());
        assert!(segfault.pre.find("ERROR SUMMARY").is_some());
        assert!(segfault.post.find("ERROR SUMMARY").is_some());
        assert!(agony.debug.pre.find("ERROR SUMMARY").is_some());
        assert!(agony.debug.post.find("ERROR SUMMARY").is_some());
        assert!(agony.release.pre.find("ERROR SUMMARY").is_some());
        assert!(agony.release.post.find("ERROR SUMMARY").is_some());

        assert!(infinite.pre.find("/root/genos_tests/infinite").is_some());
        assert!(infinite.pre.find("(in infinite)").is_none());
        assert!(infinite.post.find("/root/genos_tests/infinite").is_none());
        assert!(infinite.post.find("(in infinite)").is_some());

        assert!(segfault.pre.find("/root/genos_tests/segfault").is_some());
        assert!(segfault.pre.find("(in segfault)").is_none());
        assert!(segfault.post.find("/root/genos_tests/segfault").is_none());
        assert!(segfault.post.find("(in segfault)").is_some());

        assert!(agony
            .debug
            .pre
            .find("/usr/lib/x86_64-linux-gnu/valgrind/vgpreload_memcheck-amd64-linux.so")
            .is_some());
        assert!(agony
            .debug
            .post
            .find("/usr/lib/x86_64-linux-gnu/valgrind/vgpreload_memcheck-amd64-linux.so")
            .is_none());
        assert!(agony
            .debug
            .pre
            .find("(in vgpreload_memcheck-amd64-linux.so)")
            .is_none());
        assert!(agony
            .debug
            .post
            .find("(in vgpreload_memcheck-amd64-linux.so)")
            .is_some());

        assert!(agony
            .release
            .pre
            .find("/root/genos_tests/many/layers/of/agony")
            .is_some());
        assert!(agony
            .release
            .post
            .find("/root/genos_tests/many/layers/of/agony")
            .is_none());
        assert!(agony
            .release
            .pre
            .find("/usr/lib/x86_64-linux-gnu/valgrind/vgpreload_memcheck-amd64-linux.so")
            .is_some());
        assert!(agony
            .release
            .post
            .find("/usr/lib/x86_64-linux-gnu/valgrind/vgpreload_memcheck-amd64-linux.so")
            .is_none());
        assert!(agony
            .release
            .pre
            .find("(in vgpreload_memcheck-amd64-linux.so)")
            .is_none());
        assert!(agony
            .release
            .post
            .find("(in vgpreload_memcheck-amd64-linux.so)")
            .is_some());
        assert!(agony.release.pre.find("(in agony)").is_none());
        assert!(agony.release.post.find("(in agony)").is_some());
        // }}}
    }

    async fn mock_run(
        config: ValgrindConfig,
        exec: &str,
        ws: MockDir,
        estatus: ExitStatus,
    ) -> Result<StageResult> {
        mock_valgrind(config, exec, Vec::new(), None, None, estatus)
            .run(ws.root.path())
            .await
    }

    #[tokio::test]
    async fn run_asserts_bin_exists() {
        assert!(mock_run(
            ValgrindConfig::new("vg.log"),
            "noop",
            MockDir::new()
                .file(("noop", ""))
                .file(("vg.log", get_examples().await.unwrap().noop.clone())),
            ExitStatus::Ok
        )
        .await
        .is_ok());

        assert!(mock_run(
            ValgrindConfig::new("vg.log"),
            "infinite",
            MockDir::new()
                .file(("noop", ""))
                .file(("vg.log", get_examples().await.unwrap().noop.clone())),
            ExitStatus::Ok
        )
        .await
        .is_err());

        assert!(mock_run(
            ValgrindConfig::new("vg.log"),
            "noop",
            MockDir::new().file(("vg.log", get_examples().await.unwrap().noop.clone())),
            ExitStatus::Ok
        )
        .await
        .is_err());
    }

    #[tokio::test]
    async fn run_asserts_supp_exists() {
        {
            let mut config = ValgrindConfig::new("vg.log");
            config.suppressions = Some(vec!["foo.supp".to_string()]);
            assert!(mock_run(
                config,
                "noop",
                MockDir::new()
                    .file(("noop", ""))
                    .file(("foo.supp", ""))
                    .file(("vg.log", get_examples().await.unwrap().noop.clone())),
                ExitStatus::Ok
            )
            .await
            .is_ok());
        }

        {
            let mut config = ValgrindConfig::new("vg.log");
            config.suppressions = Some(vec!["foo.supp".to_string()]);
            assert!(mock_run(
                config,
                "noop",
                MockDir::new()
                    .file(("noop", ""))
                    .file(("vg.log", get_examples().await.unwrap().noop.clone())),
                ExitStatus::Ok
            )
            .await
            .is_err());
        }

        {
            let mut config = ValgrindConfig::new("vg.log");
            config.suppressions = Some(vec!["foo.supp".to_string(), "bar.supp".to_string()]);
            assert!(mock_run(
                config,
                "noop",
                MockDir::new()
                    .file(("noop", ""))
                    .file(("foo.supp", ""))
                    .file(("vg.log", get_examples().await.unwrap().noop.clone())),
                ExitStatus::Ok
            )
            .await
            .is_err());
        }
    }

    #[tokio::test]
    async fn run_asserts_log_nonempty() {
        assert!(mock_run(
            ValgrindConfig::new("vg.log"),
            "noop",
            MockDir::new()
                .file(("noop", ""))
                .file(("vg.log", get_examples().await.unwrap().noop.clone())),
            ExitStatus::Ok
        )
        .await
        .is_ok());

        assert!(mock_run(
            ValgrindConfig::new("vg.log"),
            "noop",
            MockDir::new().file(("noop", "")),
            ExitStatus::Ok
        )
        .await
        .is_err());

        assert!(mock_run(
            ValgrindConfig::new("vg.log"),
            "noop",
            MockDir::new().file(("noop", "")).file(("vg.log", "\n")),
            ExitStatus::Ok
        )
        .await
        .is_err());
    }

    #[tokio::test]
    async fn run_validates_exit_code() {
        assert!(mock_run(
            ValgrindConfig::new("vg.log"),
            "segfault",
            MockDir::new()
                .file(("segfault", ""))
                .file(("vg.log", get_examples().await.unwrap().segfault.pre.clone())),
            ExitStatus::Failure(ERROR_EXITCODE),
        )
        .await
        .is_ok());

        assert!(mock_run(
            ValgrindConfig::new("vg.log"),
            "segfault",
            MockDir::new()
                .file(("segfault", ""))
                .file(("vg.log", get_examples().await.unwrap().segfault.pre.clone())),
            ExitStatus::Failure(ERROR_EXITCODE + 1),
        )
        .await
        .is_ok());

        assert!(mock_run(
            ValgrindConfig::new("vg.log"),
            "segfault",
            MockDir::new()
                .file(("segfault", ""))
                .file(("vg.log", get_examples().await.unwrap().segfault.pre.clone())),
            ExitStatus::Failure(ERROR_EXITCODE - 1),
        )
        .await
        .is_err());
    }

    #[tokio::test]
    async fn run_returns_expected_status() {
        assert_eq!(
            mock_run(
                ValgrindConfig::new("vg.log"),
                "noop",
                MockDir::new()
                    .file(("noop", ""))
                    .file(("vg.log", get_examples().await.unwrap().noop.clone())),
                ExitStatus::Ok,
            )
            .await
            .unwrap()
            .status,
            StageStatus::Continue {
                points_lost: PointQuantity::zero()
            }
        );

        assert_eq!(
            mock_run(
                ValgrindConfig::new("vg.log"),
                "segfault",
                MockDir::new()
                    .file(("segfault", ""))
                    .file(("vg.log", get_examples().await.unwrap().segfault.pre.clone())),
                ExitStatus::Failure(ERROR_EXITCODE),
            )
            .await
            .unwrap()
            .status,
            StageStatus::UnrecoverableFailure
        );

        assert_eq!(
            mock_run(
                ValgrindConfig::new("vg.log"),
                "segfault",
                MockDir::new()
                    .file(("segfault", ""))
                    .file(("vg.log", get_examples().await.unwrap().segfault.pre.clone())),
                ExitStatus::Signal(SignalType::Abort),
            )
            .await
            .unwrap()
            .status,
            StageStatus::UnrecoverableFailure
        );

        assert_eq!(
            mock_run(
                ValgrindConfig::new("vg.log"),
                "segfault",
                MockDir::new()
                    .file(("segfault", ""))
                    .file(("vg.log", get_examples().await.unwrap().segfault.pre.clone())),
                ExitStatus::Signal(SignalType::SegFault),
            )
            .await
            .unwrap()
            .status,
            StageStatus::UnrecoverableFailure
        );

        // sanity check {{{
        match SignalType::Abort {
            // must update this test if SignalType changes
            SignalType::Abort => {}
            SignalType::SegFault => {}
        }
        // }}}
    }

    #[tokio::test]
    async fn run_outputs_expected_information() {
        {
            let out = mock_run(
                ValgrindConfig::new("vg.log"),
                "noop",
                MockDir::new()
                    .file(("noop", ""))
                    .file(("vg.log", get_examples().await.unwrap().noop.clone())),
                ExitStatus::Ok,
            )
            .await
            .unwrap()
            .output
            .unwrap();
            assert!(out.contains("Valgrind"));
            assert!(out.contains("running valgrind"));
            assert!(out.contains("Output:"));
            assert!(out.contains("Memcheck, a memory error detector"));
            assert!(out.contains("HEAP SUMMARY"));
            assert!(out.contains("All heap blocks were freed -- no leaks are possible"));
            assert!(out.contains("ERROR SUMMARY: 0 errors from 0 contexts"));
        }

        {
            let out = mock_run(
                ValgrindConfig::new("vg.log"),
                "segfault",
                MockDir::new()
                    .file(("segfault", ""))
                    .file(("vg.log", get_examples().await.unwrap().segfault.pre.clone())),
                ExitStatus::Signal(SignalType::SegFault),
            )
            .await
            .unwrap()
            .output
            .unwrap();
            assert!(out.contains("Valgrind"));
            assert!(out.contains("running valgrind"));
            assert!(out.contains("Output:"));
            assert!(out.contains("Memcheck, a memory error detector"));
            assert!(out.contains("HEAP SUMMARY"));
            assert!(out.contains("All heap blocks were freed -- no leaks are possible"));
            assert!(out.contains("ERROR SUMMARY: 1 errors from 1 contexts"));
            assert!(out.contains("Invalid read of size 4"));
            assert!(out.contains("(in segfault)"));
            assert!(out.contains("Your submission was killed by SIGSEGV!"));
        }

        {
            let out = mock_run(
                ValgrindConfig::new("vg.log"),
                "infinite",
                MockDir::new()
                    .file(("infinite", ""))
                    .file(("vg.log", get_examples().await.unwrap().infinite.pre.clone())),
                ExitStatus::Signal(SignalType::Abort),
            )
            .await
            .unwrap()
            .output
            .unwrap();
            assert!(out.contains("Valgrind"));
            assert!(out.contains("running valgrind"));
            assert!(out.contains("Output:"));
            assert!(out.contains("Memcheck, a memory error detector"));
            assert!(out.contains("HEAP SUMMARY"));
            assert!(out.contains("All heap blocks were freed -- no leaks are possible"));
            assert!(out.contains("ERROR SUMMARY: 0 errors from 0 contexts"));
            assert!(out.contains("Process terminating with default action of signal 6"));
            assert!(out.contains("(in infinite)"));
            assert!(out.contains("Your submission was killed by SIGABRT!"));
        }

        {
            let out = mock_run(
                ValgrindConfig::new("vg.log"),
                "infinite",
                MockDir::new()
                    .file(("infinite", ""))
                    .file(("vg.log", get_examples().await.unwrap().segfault.pre.clone())),
                ExitStatus::Timeout(DEFAULT_TIMEOUT),
            )
            .await
            .unwrap()
            .output
            .unwrap();
            assert!(out.contains("Valgrind"));
            assert!(out.contains("running valgrind"));
            assert!(out.contains(format!(
                "Your submission timed out after {} seconds :(",
                DEFAULT_TIMEOUT.as_secs()
            )));

            // note: timeout should not print the output information
            assert!(!out.contains("Output:"));
            assert!(!out.contains("Memcheck, a memory error detector"));
            assert!(!out.contains("HEAP SUMMARY"));
            assert!(!out.contains("All heap blocks were freed -- no leaks are possible"));
            assert!(!out.contains("ERROR SUMMARY: 0 errors from 0 contexts"));
            assert!(!out.contains("Process terminating with default action of signal 2 (SIGINT)"));
        }

        // sanity check {{{
        match SignalType::Abort {
            // must update this test if SignalType changes
            SignalType::Abort => {}
            SignalType::SegFault => {}
        }
        // }}}
    }

    #[tokio::test]
    async fn outputs_expected_format() {
        {
            let out = mock_run(
                ValgrindConfig::new("vg.log"),
                "noop",
                MockDir::new()
                    .file(("noop", ""))
                    .file(("vg.log", get_examples().await.unwrap().noop.clone())),
                ExitStatus::Ok,
            )
            .await
            .unwrap()
            .output
            .unwrap()
            .transform(&MarkdownFormatter);

            assert!(out.contains("# Valgrind"));
            assert!(out.contains("## Run Command"));
            assert!(out.contains("valgrind --log-file=vg.log --error-exitcode=125 -- noop"));
            assert!(out.contains("running valgrind .... pass"));
            assert!(out.contains("## Output:"));
        }

        {
            let out = mock_run(
                ValgrindConfig::new("vg.log"),
                "segfault",
                MockDir::new()
                    .file(("segfault", ""))
                    .file(("vg.log", get_examples().await.unwrap().segfault.pre.clone())),
                ExitStatus::Signal(SignalType::SegFault),
            )
            .await
            .unwrap()
            .output
            .unwrap()
            .transform(&MarkdownFormatter);

            assert!(out.contains("# Valgrind"));
            assert!(out.contains("## Run Command"));
            assert!(out.contains("valgrind --log-file=vg.log --error-exitcode=125 -- segfault"));
            assert!(out.contains("running valgrind .... fail (-fullpoints)"));
            assert!(out.contains("## feedback for running valgrind"));
            assert!(out.contains("Your submission was killed by SIGSEGV!"));
            assert!(out.contains("## Output:"));
        }

        {
            let out = mock_run(
                ValgrindConfig::new("vg.log"),
                "infinite",
                MockDir::new()
                    .file(("infinite", ""))
                    .file(("vg.log", get_examples().await.unwrap().infinite.pre.clone())),
                ExitStatus::Signal(SignalType::Abort),
            )
            .await
            .unwrap()
            .output
            .unwrap()
            .transform(&MarkdownFormatter);

            assert!(out.contains("# Valgrind"));
            assert!(out.contains("## Run Command"));
            assert!(out.contains("valgrind --log-file=vg.log --error-exitcode=125 -- infinite"));
            assert!(out.contains("running valgrind .... fail (-fullpoints)"));
            assert!(out.contains("## feedback for running valgrind"));
            assert!(out.contains("Your submission was killed by SIGABRT!"));
            assert!(out.contains("## Output:"));
        }

        {
            let out = mock_run(
                ValgrindConfig::new("vg.log"),
                "infinite",
                MockDir::new()
                    .file(("infinite", ""))
                    .file(("vg.log", get_examples().await.unwrap().infinite.pre.clone())),
                ExitStatus::Timeout(DEFAULT_TIMEOUT),
            )
            .await
            .unwrap()
            .output
            .unwrap()
            .transform(&MarkdownFormatter);

            assert!(out.contains("# Valgrind"));
            assert!(out.contains("## Run Command"));
            assert!(out.contains("valgrind --log-file=vg.log --error-exitcode=125 -- infinite"));
            assert!(out.contains("running valgrind .... fail (-fullpoints)"));
            assert!(out.contains("## feedback for running valgrind"));
            assert!(out.contains(&format!(
                "Your submission timed out after {} seconds :(",
                DEFAULT_TIMEOUT.as_secs()
            )));
            //assert!(out.contains("## Output:"));
        }
    }
}

// vim: fdm=marker fmr={{{,}}}
