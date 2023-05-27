use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{anyhow, Result};
use async_trait::async_trait;

use genos::{
    fs::read_file,
    output::{Content, Output, RichTextMaker, Section, StatusUpdates, Update},
    points::PointQuantity,
    process::{Command, ExitStatus, ProcessExecutor, SignalType, StdinPipe},
    stage::{StageResult, StageStatus},
    tid::TestId,
    Executor,
};

use regex::Regex;

use serde::Deserialize;

use tracing::debug;

use crate::finder::{Finder, TestResourceFinder};

use super::run::RunConfig;

// name of the required log file
const LOG_FILE: &'static str = "valgrind.log";

// some arbitrary bytes to fill for malloc and free
const MALLOC_FILL: u8 = 0xF0;
const FREE_FILL: u8 = 0x0B;

// default should be longer than run stage default
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);
const TIMEOUT_MULT: f32 = 2.0; // in relation to run config

// exit code used to identify non-signal failures.
// not configurable via YAML.
//
// exit codes >125 are reserved by POSIX; see exit(1p)
// and https://tldp.org/LDP/abs/html/exitcodes.html
const ERROR_EXITCODE: i32 = 125;

#[derive(Debug, Deserialize, Clone)]
pub struct ValgrindConfig {
    points: PointQuantity,
    suppressions: Option<Vec<String>>,
}

pub struct Valgrind<E> {
    pub executor: E,
    pub config: ValgrindConfig,
    pub run_config: RunConfig,
    pub config_path: PathBuf,
    pub test_id: TestId,
}

// replaces all absolute paths with basenames
fn hide_absolute_paths(s: &str) -> Result<String> {
    let re = Regex::new(r"(\W|^)(?:\/[^\/\s]+)+\/([^\/\s]+)\b")?;
    let repl = re.replace_all(s, "$1$2");

    Ok(repl.to_string())
}

impl<E: ProcessExecutor> Valgrind<E> {
    fn gen_cmd(&self, ws: &Path) -> Result<Command> {
        let mut cmd = Command::new("valgrind")
            .arg(format!("--log-file={}", LOG_FILE))
            .arg(format!("--leak-check=yes"))
            .arg(format!("--error-exitcode={}", ERROR_EXITCODE))
            .arg(format!("--malloc-fill=0x{:02X}", MALLOC_FILL))
            .arg(format!("--free-fill=0x{:02X}", FREE_FILL));

        if let Some(v) = &self.config.suppressions {
            let finder = Finder::from_hw_config_path(self.config_path.as_path())?;
            for supp in v {
                let path = finder.test_resource(self.test_id, supp)?;
                if let Some(v) = path.to_str() {
                    cmd.add_arg(format!("--suppressions={}", v));
                } else {
                    return Err(anyhow!("Invalid suppression path {}", path.display()));
                }
            }
        }

        cmd.add_arg("--");
        let exec = self.run_config.executable(ws)?;
        if let Some(v) = exec.to_str() {
            cmd.add_arg(v);
        } else {
            return Err(anyhow!("Invalid executable path {}", exec.display()));
        }

        cmd.add_args(&self.run_config.args);
        if let Some(v) = &self.run_config.stdin {
            cmd.set_stdin(StdinPipe::Path(v.into()))
        }

        cmd.set_timeout(
            self.run_config
                .timeout()
                .map(|v| Duration::from_secs_f32(v.as_secs_f32() * TIMEOUT_MULT))
                .unwrap_or(DEFAULT_TIMEOUT),
        );
        cmd.set_cwd(ws);

        Ok(cmd)
    }

    async fn read_logfile(&self, ws: &Path) -> Result<String> {
        let path = ws.join(LOG_FILE);
        if !path.exists() {
            return Err(anyhow!(
                "Could not find logfile at {:?}. Did Valgrind actually execute?",
                path.display()
            ));
        }
        let contents = read_file(&path).await?;
        if contents.trim().is_empty() {
            return Err(anyhow!(
                "Found empty valgrind log at {:?}. Something went wrong.",
                path.display()
            ));
        }

        Ok(hide_absolute_paths(contents.as_str())?)
    }
}

#[async_trait]
impl<E: ProcessExecutor> Executor for Valgrind<E> {
    type Output = StageResult;

    async fn run(&self, ws: &Path) -> Result<Self::Output> {
        let mut sect = Section::new("Valgrind");
        let mut results_sect = Section::new("Output");
        let mut run_updates = StatusUpdates::default();
        debug!("running valgrind");

        let cmd = self.gen_cmd(ws)?;
        sect.add_content((
            "Run Command",
            hide_absolute_paths(format!("{}", cmd).as_str())?.code(),
        ));

        let res = cmd.run_with(&self.executor).await?;

        let log = self.read_logfile(ws).await?;

        if !res.status.completed() {
            match res.status {
                ExitStatus::Timeout(to) => {
                    run_updates.add_update(
                        Update::new_fail("running valgrind", self.config.points).notes(format!(
                            "Your submission timed out after {} second{} :(",
                            to.as_secs(),
                            if to.as_secs() == 1 { "" } else { "s" }
                        )),
                    );
                    sect.add_content(run_updates);
                }
                ExitStatus::Signal(sig) => {
                    run_updates.add_update(
                        Update::new_fail("running valgrind", self.config.points).notes(format!(
                            "Your submission was killed by {}!",
                            if sig == SignalType::Abort {
                                "SIGABRT"
                            } else {
                                "SIGSEGV"
                            }
                        )),
                    );
                    results_sect.add_content(log);
                    sect.add_content(run_updates);
                    sect.add_content(Content::SubSection(results_sect));

                    match sig {
                        // this procedure must be updated if the signals change
                        SignalType::SegFault => {}
                        SignalType::Abort => {}
                    }
                }
                _ => panic!("Expected either Timeout or Signal if command was not completed"),
            }
            return Ok(StageResult::new(
                StageStatus::Continue {
                    points_lost: self.config.points,
                },
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
                    Update::new_fail("running valgrind", self.config.points)
                        .notes("Valgrind Errors Detected"),
                );
                results_sect.add_content(log);
                sect.add_content(run_updates);
                sect.add_content(Content::SubSection(results_sect));

                return Ok(StageResult::new(
                    StageStatus::Continue {
                        points_lost: self.config.points,
                    },
                    Some(Output::new().section(sect)),
                ));
            }
            _ => panic!(
                "Expected either ExitStatus::Ok or ExitStatus::Failure if the command completed"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        env,
        path::PathBuf,
        sync::{Arc, Mutex},
    };

    use genos::{
        output::Contains,
        process::{self, is_program_in_path, ShellExecutor},
        test_util::{MockDir, MockExecutorInner, MockProcessExecutor},
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

    const TEST_MAIN_PATH: &'static str = "resources/valgrind/main.c";
    const EXAMPLES_PATH: &'static str = "resources/valgrind/examples.yaml";
    static EXAMPLES: OnceCell<Examples> = OnceCell::const_new();

    async fn read_examples() -> Result<Examples> {
        let mut examples_in = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        examples_in.push(EXAMPLES_PATH);
        assert!(
            examples_in.exists(),
            "Expected to find valgrind examples at {}",
            examples_in.display()
        );

        Ok(
            serde_yaml::from_str::<Examples>(&read_file(examples_in.as_path()).await.unwrap())
                .unwrap(),
        )
    }

    // access loaded examples
    async fn get_examples() -> &'static Examples {
        EXAMPLES.get_or_try_init(read_examples).await.unwrap()
    }

    fn mock_ws(
        root_files: Vec<(&str, &str)>,
        static_files: Vec<(&str, &str)>,
        test_files: Vec<(&str, &str)>,
    ) -> MockDir {
        let ws = MockDir::new();
        let static_dir = MockDir::new();
        let test_dir = MockDir::new();
        for v in root_files {
            ws.add_file((v.0, v.1));
        }
        for v in static_files {
            static_dir.add_file((v.0, v.1));
        }
        for v in test_files {
            test_dir.add_file((v.0, v.1));
        }
        ws.dir(
            "data",
            MockDir::new()
                .dir("system", MockDir::new().file(("genos_unittest.h", "")))
                .dir(
                    "course",
                    MockDir::new().dir(
                        "hw1",
                        MockDir::new()
                            .file(("hw.yaml", ""))
                            .dir("static", static_dir)
                            .dir("test_1", test_dir),
                    ),
                ),
        )
    }

    fn mock_valgrind(
        config: ValgrindConfig,
        exec: &str,
        args: Vec<String>,
        stdin: Option<String>,
        timeout: Option<Duration>,
        estatus: ExitStatus,
        ws: &MockDir,
    ) -> Valgrind<MockProcessExecutor> {
        Valgrind {
            executor: MockProcessExecutor::new(Arc::new(Mutex::new(
                MockExecutorInner::with_responses([Ok(process::Output::from_exit_status(estatus))]),
            ))),
            config,
            run_config: RunConfig {
                executable: exec.to_string(),
                args,
                stdin,
                timeout_sec: timeout.map(|v| v.as_secs()),
                ..RunConfig::default()
            },
            config_path: ws.path_from_root("data/course/hw1/hw.yaml"),
            test_id: TestId::new(1),
        }
    }

    fn mock_cmd(
        config: ValgrindConfig,
        exec: &str,
        stdin: Option<String>,
        estatus: ExitStatus,
        ws: &MockDir,
    ) -> Result<String> {
        Ok(
            mock_valgrind(config, exec, Vec::new(), stdin, None, estatus, ws)
                .gen_cmd(ws.root.path())?
                .to_string(),
        )
    }

    #[tokio::test]
    async fn cmd_reflects_inputs() {
        let ws = mock_ws(
            vec![("noop", "")],
            vec![("static.supp", "")],
            vec![("test.supp", "")],
        );

        assert_eq!(
            mock_cmd(
                ValgrindConfig {
                    points: PointQuantity::FullPoints,
                    suppressions: None
                },
                "noop",
                None,
                ExitStatus::Ok,
                &ws,
            )
            .unwrap(),
            format!(
                "valgrind --log-file={} --leak-check=yes --error-exitcode={} \
                 --malloc-fill=0x{:02X} --free-fill=0x{:02X} -- {}",
                LOG_FILE,
                ERROR_EXITCODE,
                MALLOC_FILL,
                FREE_FILL,
                ws.path_from_root("noop").display()
            )
        );

        assert_eq!(
            mock_cmd(
                ValgrindConfig {
                    points: PointQuantity::FullPoints,
                    suppressions: None
                },
                "noop",
                Some("bar".to_string()),
                ExitStatus::Ok,
                &ws
            )
            .unwrap(),
            format!(
                "valgrind --log-file={} --leak-check=yes --error-exitcode={} \
                 --malloc-fill=0x{:02X} --free-fill=0x{:02X} -- {} < bar",
                LOG_FILE,
                ERROR_EXITCODE,
                MALLOC_FILL,
                FREE_FILL,
                ws.path_from_root("noop").display()
            )
        );

        assert_eq!(
            mock_cmd(
                ValgrindConfig {
                    points: PointQuantity::FullPoints,
                    suppressions: Some(vec!["static.supp".to_string()])
                },
                "noop",
                Some("bar".to_string()),
                ExitStatus::Ok,
                &ws,
            )
            .unwrap(),
            format!(
                "valgrind --log-file={} --leak-check=yes --error-exitcode={} \
                 --malloc-fill=0x{:02X} --free-fill=0x{:02X} --suppressions={} \
                 -- {} < bar",
                LOG_FILE,
                ERROR_EXITCODE,
                MALLOC_FILL,
                FREE_FILL,
                ws.path_from_root("data/course/hw1/static/static.supp")
                    .display(),
                ws.path_from_root("noop").display()
            )
        );

        assert_eq!(
            mock_cmd(
                ValgrindConfig {
                    points: PointQuantity::FullPoints,
                    suppressions: Some(vec!["static.supp".to_string(), "test.supp".to_string()])
                },
                "noop",
                Some("bar".to_string()),
                ExitStatus::Ok,
                &ws,
            )
            .unwrap(),
            format!(
                "valgrind --log-file={} --leak-check=yes --error-exitcode={} \
                 --malloc-fill=0x{:02X} --free-fill=0x{:02X} --suppressions={} \
                 --suppressions={} -- {} < bar",
                LOG_FILE,
                ERROR_EXITCODE,
                MALLOC_FILL,
                FREE_FILL,
                ws.path_from_root("data/course/hw1/static/static.supp")
                    .display(),
                ws.path_from_root("data/course/hw1/test_1/test.supp")
                    .display(),
                ws.path_from_root("noop").display()
            )
        );
    }

    #[tokio::test]
    async fn cmd_asserts_bin_exists() {
        let ws = mock_ws(
            vec![("noop", "")],
            vec![("static.supp", "")],
            vec![("test.supp", "")],
        );

        assert!(mock_cmd(
            ValgrindConfig {
                points: PointQuantity::FullPoints,
                suppressions: None
            },
            "noop",
            None,
            ExitStatus::Ok,
            &ws,
        )
        .is_ok());

        assert!(mock_cmd(
            ValgrindConfig {
                points: PointQuantity::FullPoints,
                suppressions: None
            },
            "nope",
            None,
            ExitStatus::Ok,
            &ws,
        )
        .is_err());
    }

    #[tokio::test]
    async fn cmd_asserts_supp_exists() {
        let ws = mock_ws(
            vec![("noop", "")],
            vec![("static.supp", "")],
            vec![("test.supp", "")],
        );

        assert!(mock_cmd(
            ValgrindConfig {
                points: PointQuantity::FullPoints,
                suppressions: Some(vec!["static.supp".to_string()])
            },
            "noop",
            None,
            ExitStatus::Ok,
            &ws,
        )
        .is_ok());

        assert!(mock_cmd(
            ValgrindConfig {
                points: PointQuantity::FullPoints,
                suppressions: Some(vec!["static.soap".to_string()])
            },
            "noop",
            None,
            ExitStatus::Ok,
            &ws,
        )
        .is_err());

        assert!(mock_cmd(
            ValgrindConfig {
                points: PointQuantity::FullPoints,
                suppressions: Some(vec!["static.supp".to_string(), "test.supp".to_string()])
            },
            "noop",
            None,
            ExitStatus::Ok,
            &ws,
        )
        .is_ok());

        assert!(mock_cmd(
            ValgrindConfig {
                points: PointQuantity::FullPoints,
                suppressions: Some(vec!["static.supp".to_string(), "test.soap".to_string()])
            },
            "noop",
            None,
            ExitStatus::Ok,
            &ws,
        )
        .is_err());
    }

    async fn mock_read(config: ValgrindConfig, exec: &str, ws: &MockDir) -> Result<String> {
        mock_valgrind(config, exec, Vec::new(), None, None, ExitStatus::Ok, ws)
            .read_logfile(ws.root.path())
            .await
    }

    #[tokio::test]
    async fn read_validates_logfile() {
        assert!(mock_read(
            ValgrindConfig {
                points: PointQuantity::FullPoints,
                suppressions: None
            },
            "noop",
            &mock_ws(vec![("noop", "")], vec![], vec![]),
        )
        .await
        .is_err());

        assert!(mock_read(
            ValgrindConfig {
                points: PointQuantity::FullPoints,
                suppressions: None
            },
            "noop",
            &mock_ws(vec![("noop", ""), ("valgrind.log", "")], vec![], vec![]),
        )
        .await
        .is_err());

        assert!(mock_read(
            ValgrindConfig {
                points: PointQuantity::FullPoints,
                suppressions: None
            },
            "noop",
            &mock_ws(vec![("noop", ""), ("valgrind.log", "\n\n")], vec![], vec![]),
        )
        .await
        .is_err());

        assert!(mock_read(
            ValgrindConfig {
                points: PointQuantity::FullPoints,
                suppressions: None
            },
            "noop",
            &mock_ws(
                vec![
                    ("noop", ""),
                    ("valgrind.log", get_examples().await.noop.as_str())
                ],
                vec![],
                vec![]
            ),
        )
        .await
        .is_ok());

        assert!(mock_read(
            ValgrindConfig {
                points: PointQuantity::FullPoints,
                suppressions: None
            },
            "noop",
            &mock_ws(
                vec![
                    ("infinite", ""),
                    ("valgrind.log", get_examples().await.infinite.pre.as_str())
                ],
                vec![],
                vec![]
            ),
        )
        .await
        .is_ok());
    }

    #[tokio::test]
    async fn read_hides_absolute_paths() {
        let noop = &get_examples().await.noop;
        let infinite = &get_examples().await.infinite;
        let segfault = &get_examples().await.segfault;
        let agony = &get_examples().await.agony;

        assert_eq!(
            mock_read(
                ValgrindConfig {
                    points: PointQuantity::FullPoints,
                    suppressions: None
                },
                "noop",
                &mock_ws(
                    vec![("noop", ""), ("valgrind.log", noop.as_str())],
                    vec![],
                    vec![]
                ),
            )
            .await
            .unwrap(),
            noop.clone()
        );

        assert_eq!(
            mock_read(
                ValgrindConfig {
                    points: PointQuantity::FullPoints,
                    suppressions: None
                },
                "infinite",
                &mock_ws(
                    vec![("infinite", ""), ("valgrind.log", infinite.pre.as_str())],
                    vec![],
                    vec![]
                ),
            )
            .await
            .unwrap(),
            infinite.post.clone()
        );

        assert_eq!(
            mock_read(
                ValgrindConfig {
                    points: PointQuantity::FullPoints,
                    suppressions: None
                },
                "segfault",
                &mock_ws(
                    vec![("segfault", ""), ("valgrind.log", segfault.pre.as_str())],
                    vec![],
                    vec![]
                ),
            )
            .await
            .unwrap(),
            segfault.post.clone()
        );

        assert_eq!(
            mock_read(
                ValgrindConfig {
                    points: PointQuantity::FullPoints,
                    suppressions: None
                },
                "agony",
                &mock_ws(
                    vec![("agony", ""), ("valgrind.log", agony.debug.pre.as_str())],
                    vec![],
                    vec![]
                ),
            )
            .await
            .unwrap(),
            agony.debug.post.clone()
        );

        assert_eq!(
            mock_read(
                ValgrindConfig {
                    points: PointQuantity::FullPoints,
                    suppressions: None
                },
                "agony",
                &mock_ws(
                    vec![("agony", ""), ("valgrind.log", agony.release.pre.as_str())],
                    vec![],
                    vec![]
                ),
            )
            .await
            .unwrap(),
            agony.release.post.clone()
        );

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
    }

    async fn mock_run(
        config: ValgrindConfig,
        exec: &str,
        estatus: ExitStatus,
        ws: &MockDir,
    ) -> Result<StageResult> {
        mock_valgrind(config, exec, Vec::new(), None, None, estatus, ws)
            .run(ws.root.path())
            .await
    }

    #[tokio::test]
    async fn run_asserts_bin_exists() {
        let ws = mock_ws(
            vec![
                ("noop", ""),
                ("valgrind.log", get_examples().await.noop.as_str()),
            ],
            vec![],
            vec![],
        );

        assert!(mock_run(
            ValgrindConfig {
                points: PointQuantity::FullPoints,
                suppressions: None
            },
            "noop",
            ExitStatus::Ok,
            &ws,
        )
        .await
        .is_ok());

        assert!(mock_run(
            ValgrindConfig {
                points: PointQuantity::FullPoints,
                suppressions: None
            },
            "nope",
            ExitStatus::Ok,
            &ws,
        )
        .await
        .is_err());
    }

    #[tokio::test]
    async fn run_asserts_supp_exists() {
        let ws = mock_ws(
            vec![
                ("noop", ""),
                ("valgrind.log", get_examples().await.noop.as_str()),
            ],
            vec![("foo.supp", "")],
            vec![("bar.supp", "")],
        );

        assert!(mock_run(
            ValgrindConfig {
                points: PointQuantity::FullPoints,
                suppressions: Some(vec!["foo.supp".to_string()])
            },
            "noop",
            ExitStatus::Ok,
            &ws,
        )
        .await
        .is_ok());

        assert!(mock_run(
            ValgrindConfig {
                points: PointQuantity::FullPoints,
                suppressions: Some(vec!["foo.soap".to_string()])
            },
            "noop",
            ExitStatus::Ok,
            &ws,
        )
        .await
        .is_err());

        assert!(mock_run(
            ValgrindConfig {
                points: PointQuantity::FullPoints,
                suppressions: Some(vec!["foo.supp".to_string(), "bar.supp".to_string()])
            },
            "noop",
            ExitStatus::Ok,
            &ws,
        )
        .await
        .is_ok());

        assert!(mock_run(
            ValgrindConfig {
                points: PointQuantity::FullPoints,
                suppressions: Some(vec!["foo.soap".to_string(), "bar.supp".to_string()])
            },
            "noop",
            ExitStatus::Ok,
            &ws,
        )
        .await
        .is_err());

        assert!(mock_run(
            ValgrindConfig {
                points: PointQuantity::FullPoints,
                suppressions: Some(vec!["foo.supp".to_string(), "bar.soap".to_string()])
            },
            "noop",
            ExitStatus::Ok,
            &ws,
        )
        .await
        .is_err());

        assert!(mock_run(
            ValgrindConfig {
                points: PointQuantity::FullPoints,
                suppressions: Some(vec!["foo.soap".to_string(), "bear.supp".to_string()])
            },
            "noop",
            ExitStatus::Ok,
            &ws,
        )
        .await
        .is_err());
    }

    #[tokio::test]
    async fn run_asserts_log_nonempty() {
        assert!(mock_run(
            ValgrindConfig {
                points: PointQuantity::FullPoints,
                suppressions: None
            },
            "noop",
            ExitStatus::Ok,
            &mock_ws(
                vec![
                    ("noop", ""),
                    ("valgrind.log", get_examples().await.noop.as_str()),
                ],
                vec![],
                vec![],
            ),
        )
        .await
        .is_ok());

        assert!(mock_run(
            ValgrindConfig {
                points: PointQuantity::FullPoints,
                suppressions: None
            },
            "noop",
            ExitStatus::Ok,
            &mock_ws(vec![("noop", "")], vec![], vec![]),
        )
        .await
        .is_err());

        assert!(mock_run(
            ValgrindConfig {
                points: PointQuantity::FullPoints,
                suppressions: None
            },
            "noop",
            ExitStatus::Ok,
            &mock_ws(vec![("noop", ""), ("valgrind.log", "")], vec![], vec![]),
        )
        .await
        .is_err());

        assert!(mock_run(
            ValgrindConfig {
                points: PointQuantity::FullPoints,
                suppressions: None
            },
            "noop",
            ExitStatus::Ok,
            &mock_ws(vec![("noop", ""), ("valgrind.log", "\n\n")], vec![], vec![]),
        )
        .await
        .is_err());
    }

    #[tokio::test]
    async fn run_validates_exit_code() {
        let ws = mock_ws(
            vec![
                ("segfault", ""),
                ("valgrind.log", get_examples().await.segfault.pre.as_str()),
            ],
            vec![],
            vec![],
        );

        assert!(mock_run(
            ValgrindConfig {
                points: PointQuantity::FullPoints,
                suppressions: None
            },
            "segfault",
            ExitStatus::Failure(ERROR_EXITCODE),
            &ws,
        )
        .await
        .is_ok());

        assert!(mock_run(
            ValgrindConfig {
                points: PointQuantity::FullPoints,
                suppressions: None
            },
            "segfault",
            ExitStatus::Failure(ERROR_EXITCODE + 1),
            &ws,
        )
        .await
        .is_ok());

        assert!(mock_run(
            ValgrindConfig {
                points: PointQuantity::FullPoints,
                suppressions: None
            },
            "segfault",
            ExitStatus::Failure(ERROR_EXITCODE - 1),
            &ws,
        )
        .await
        .is_err());
    }

    #[tokio::test]
    async fn run_returns_expected_status() {
        assert_eq!(
            mock_run(
                ValgrindConfig {
                    points: PointQuantity::FullPoints,
                    suppressions: None
                },
                "noop",
                ExitStatus::Ok,
                &mock_ws(
                    vec![
                        ("noop", ""),
                        ("valgrind.log", get_examples().await.noop.as_str()),
                    ],
                    vec![],
                    vec![],
                )
            )
            .await
            .unwrap()
            .status,
            StageStatus::Continue {
                points_lost: PointQuantity::zero()
            }
        );

        let seg_ws = mock_ws(
            vec![
                ("segfault", ""),
                ("valgrind.log", get_examples().await.segfault.pre.as_str()),
            ],
            vec![],
            vec![],
        );

        assert_eq!(
            mock_run(
                ValgrindConfig {
                    points: PointQuantity::FullPoints,
                    suppressions: None
                },
                "segfault",
                ExitStatus::Failure(ERROR_EXITCODE),
                &seg_ws,
            )
            .await
            .unwrap()
            .status,
            StageStatus::Continue {
                points_lost: PointQuantity::FullPoints,
            },
        );

        assert_eq!(
            mock_run(
                ValgrindConfig {
                    points: PointQuantity::FullPoints,
                    suppressions: None
                },
                "segfault",
                ExitStatus::Signal(SignalType::Abort),
                &seg_ws,
            )
            .await
            .unwrap()
            .status,
            StageStatus::Continue {
                points_lost: PointQuantity::FullPoints,
            },
        );

        assert_eq!(
            mock_run(
                ValgrindConfig {
                    points: PointQuantity::FullPoints,
                    suppressions: None
                },
                "segfault",
                ExitStatus::Signal(SignalType::SegFault),
                &seg_ws,
            )
            .await
            .unwrap()
            .status,
            StageStatus::Continue {
                points_lost: PointQuantity::FullPoints,
            },
        );

        match SignalType::Abort {
            // must update this test if SignalType changes
            SignalType::Abort => {}
            SignalType::SegFault => {}
        }
    }

    #[tokio::test]
    async fn run_outputs_expected_information() {
        {
            let out = mock_run(
                ValgrindConfig {
                    points: PointQuantity::FullPoints,
                    suppressions: None,
                },
                "noop",
                ExitStatus::Ok,
                &mock_ws(
                    vec![
                        ("noop", ""),
                        ("valgrind.log", get_examples().await.noop.as_str()),
                    ],
                    vec![],
                    vec![],
                ),
            )
            .await
            .unwrap()
            .output
            .unwrap();
            assert!(out.contains("Valgrind"));
            assert!(out.contains("running valgrind"));
            assert!(out.contains("Output"));
            assert!(out.contains("Memcheck, a memory error detector"));
            assert!(out.contains("HEAP SUMMARY"));
            assert!(out.contains("All heap blocks were freed -- no leaks are possible"));
            assert!(out.contains("ERROR SUMMARY: 0 errors from 0 contexts"));
        }

        {
            let out = mock_run(
                ValgrindConfig {
                    points: PointQuantity::FullPoints,
                    suppressions: None,
                },
                "segfault",
                ExitStatus::Signal(SignalType::SegFault),
                &mock_ws(
                    vec![
                        ("segfault", ""),
                        ("valgrind.log", get_examples().await.segfault.pre.as_str()),
                    ],
                    vec![],
                    vec![],
                ),
            )
            .await
            .unwrap()
            .output
            .unwrap();
            assert!(out.contains("Valgrind"));
            assert!(out.contains("running valgrind"));
            assert!(out.contains("Output"));
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
                ValgrindConfig {
                    points: PointQuantity::FullPoints,
                    suppressions: None,
                },
                "infinite",
                ExitStatus::Signal(SignalType::Abort),
                &mock_ws(
                    vec![
                        ("infinite", ""),
                        ("valgrind.log", get_examples().await.infinite.pre.as_str()),
                    ],
                    vec![],
                    vec![],
                ),
            )
            .await
            .unwrap()
            .output
            .unwrap();
            assert!(out.contains("Valgrind"));
            assert!(out.contains("running valgrind"));
            assert!(out.contains("Output"));
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
                ValgrindConfig {
                    points: PointQuantity::FullPoints,
                    suppressions: None,
                },
                "infinite",
                ExitStatus::Timeout(DEFAULT_TIMEOUT),
                &mock_ws(
                    vec![
                        ("infinite", ""),
                        ("valgrind.log", get_examples().await.infinite.pre.as_str()),
                    ],
                    vec![],
                    vec![],
                ),
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
            assert!(!out.contains("Output"));
            assert!(!out.contains("Memcheck, a memory error detector"));
            assert!(!out.contains("HEAP SUMMARY"));
            assert!(!out.contains("All heap blocks were freed -- no leaks are possible"));
            assert!(!out.contains("ERROR SUMMARY: 0 errors from 0 contexts"));
            assert!(!out.contains("Process terminating with default action of signal 2 (SIGINT)"));
        }

        match SignalType::Abort {
            // must update this test if SignalType changes
            SignalType::Abort => {}
            SignalType::SegFault => {}
        }
    }

    #[tokio::test]
    async fn run_hides_command_paths() {
        {
            let out = mock_run(
                ValgrindConfig {
                    points: PointQuantity::FullPoints,
                    suppressions: Some(vec!["static.supp".to_string()]),
                },
                "noop",
                ExitStatus::Ok,
                &mock_ws(
                    vec![
                        ("noop", ""),
                        ("valgrind.log", get_examples().await.noop.as_str()),
                    ],
                    vec![("static.supp", "")],
                    vec![],
                ),
            )
            .await
            .unwrap()
            .output
            .unwrap();
            assert!(out.contains("static.supp"));
            assert!(!out.contains("data/course/hw1/static/static.supp"));
            assert!(!out.contains("static/static.supp"));
        }

        {
            let out = mock_run(
                ValgrindConfig {
                    points: PointQuantity::FullPoints,
                    suppressions: Some(vec!["static.supp".to_string(), "test.supp".to_string()]),
                },
                "noop",
                ExitStatus::Ok,
                &mock_ws(
                    vec![
                        ("noop", ""),
                        ("valgrind.log", get_examples().await.noop.as_str()),
                    ],
                    vec![("static.supp", "")],
                    vec![("test.supp", "")],
                ),
            )
            .await
            .unwrap()
            .output
            .unwrap();
            assert!(out.contains("static.supp"));
            assert!(out.contains("test.supp"));
            assert!(!out.contains("data/course/hw1/static/static.supp"));
            assert!(!out.contains("data/course/hw1/test_1/test.supp"));
            assert!(!out.contains("static/static.supp"));
            assert!(!out.contains("test_1/test.supp"));
        }
    }

    #[tokio::test]
    async fn valgrind_binary_execution_tests_if_valgrind_installed() {
        if !is_program_in_path("valgrind") {
            return;
        }

        let ws = mock_ws(vec![], vec![], vec![]);

        let mut test_main = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        test_main.push(TEST_MAIN_PATH);
        assert!(
            test_main.exists(),
            "Expected to find valgrind test source at {}",
            test_main.display()
        );
        let src = test_main.as_path().file_name().unwrap().to_str().unwrap();

        tokio::fs::copy(test_main.as_path(), ws.path_from_root(src))
            .await
            .unwrap();

        let mut cmd = Command::new("gcc").arg(src).cwd(ws.root.path());

        // ensure GCC has access to system environment vars
        for (k, v) in env::vars() {
            cmd.add_env(k, v);
        }

        let build_result = cmd.run_with(&ShellExecutor).await.unwrap();

        assert_eq!(build_result.stdout, "");
        assert_eq!(build_result.stderr, "");
        assert_eq!(build_result.status, ExitStatus::Ok);

        fn mock_shell_vg(
            ws: &MockDir,
            args: Vec<String>,
            timeout_sec: Option<u64>,
        ) -> Valgrind<ShellExecutor> {
            Valgrind {
                executor: ShellExecutor,
                config: ValgrindConfig {
                    points: PointQuantity::Partial(42.into()),
                    suppressions: None,
                },
                run_config: RunConfig {
                    executable: "a.out".to_string(),
                    args,
                    stdin: None,
                    timeout_sec,
                    ..RunConfig::default()
                },
                config_path: ws.path_from_root("data/course/hw1/hw.yaml"),
                test_id: TestId::new(1),
            }
        }

        // test OK
        {
            let vg = mock_shell_vg(&ws, vec![], None);
            let res = vg.run(ws.root.path()).await.unwrap();
            assert_eq!(
                res.status,
                StageStatus::Continue {
                    points_lost: PointQuantity::zero()
                }
            );
        }

        // test segfault
        {
            let vg = mock_shell_vg(&ws, vec!["segfault".to_string()], None);
            let res = vg.run(ws.root.path()).await.unwrap();
            assert_eq!(
                res.status,
                StageStatus::Continue {
                    points_lost: PointQuantity::Partial(42.into())
                }
            );

            assert!(res
                .output
                .unwrap()
                .contains("Your submission was killed by SIGSEGV!"))
        }

        // test abort
        {
            let vg = mock_shell_vg(&ws, vec!["abort".to_string()], None);
            let res = vg.run(ws.root.path()).await.unwrap();
            assert_eq!(
                res.status,
                StageStatus::Continue {
                    points_lost: PointQuantity::Partial(42.into())
                }
            );

            assert!(res
                .output
                .unwrap()
                .contains("Your submission was killed by SIGABRT!"))
        }

        // test timeout
        {
            let vg = mock_shell_vg(&ws, vec!["timeout".to_string()], Some(1));
            let res = vg.run(ws.root.path()).await.unwrap();
            assert_eq!(
                res.status,
                StageStatus::Continue {
                    points_lost: PointQuantity::Partial(42.into())
                }
            );

            assert!(res.output.unwrap().contains("Your submission timed out"))
        }
    }
}
