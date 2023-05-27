// need to be able to mock running processes.
// what does a mock need?
//  - given the arguments for a process, it needs to be able to return a value

use std::{
    collections::HashMap,
    env,
    fmt::Display,
    fs,
    os::unix::process::ExitStatusExt,
    path::PathBuf,
    process::{ExitStatus as StdExitStatus, Stdio},
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result};
use async_trait::async_trait;
use futures::future::join_all;
use tokio::{
    fs::File,
    io::{copy, AsyncReadExt, AsyncSeekExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, Command as TokioCommand},
    sync::Mutex,
    task::JoinHandle,
    time::timeout,
};
use tracing::info;

/// Command is a struct which acts similar to a builder and wraps an instance of a tokio async
/// command. Once built it can be run multiple times on any given executor. An executor is a struct
/// which knows how to execute a given command.
///
/// You can think of the Command as a set of instructions and the executor knows how to execute
/// the instructions. This abstraction is useful for mocking internal dependencies where you want
/// to control the output to influence the direction some internal code will take while also not
/// having side effects on the external system. Any structs which need to run a process can then be
/// generic on the Executor which they run it on.
#[derive(Default, Clone)]
pub struct Command {
    pub program: String,
    pub args: Vec<String>,
    pub envs: HashMap<String, String>,
    pub cwd: Option<PathBuf>,
    pub stdin: Option<StdinPipe>,
    pub stderr: Option<PathBuf>,
    pub stdout: Option<PathBuf>,
    pub timeout: Option<Duration>,
}

impl Command {
    pub fn new<T: Into<String>>(program: T) -> Self {
        Self {
            program: program.into(),
            ..Default::default()
        }
    }

    pub fn add_arg<T: Into<String>>(&mut self, arg: T) {
        let arg = arg.into();
        self.args.push(arg);
    }

    pub fn arg<T: Into<String>>(mut self, arg: T) -> Self {
        self.add_arg(arg);
        self
    }

    pub fn add_args<T, S>(&mut self, args: T)
    where
        T: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args.extend(args.into_iter().map(|s| s.into()));
    }

    pub fn args<T, S>(mut self, args: T) -> Self
    where
        T: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.add_args(args);
        self
    }

    pub fn add_env<K, V>(&mut self, key: K, val: V)
    where
        K: Into<String>,
        V: Into<String>,
    {
        self.envs.insert(key.into(), val.into());
    }

    pub fn env<K, V>(mut self, key: K, val: V) -> Self
    where
        K: Into<String>,
        V: Into<String>,
    {
        self.add_env(key, val);
        self
    }

    pub fn cwd<T: Into<PathBuf>>(mut self, cwd: T) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    pub fn set_cwd<T: Into<PathBuf>>(&mut self, cwd: T) {
        self.cwd = Some(cwd.into());
    }

    pub fn stdin(mut self, cfg: StdinPipe) -> Self {
        // save the stdin type here, then run the process differently based on pipe type
        self.stdin = Some(cfg);
        self
    }

    pub fn set_stdin(&mut self, cfg: StdinPipe) {
        self.stdin = Some(cfg);
    }

    pub fn stderr<T: Into<PathBuf>>(mut self, cfg: T) -> Self {
        // save out/err here and then optionally save the output if given
        self.stderr = Some(cfg.into());
        self
    }

    pub fn set_stderr<T: Into<PathBuf>>(&mut self, cfg: T) {
        // save out/err here and then optionally save the output if given
        self.stderr = Some(cfg.into());
    }

    pub fn stdout<T: Into<PathBuf>>(mut self, cfg: T) -> Self {
        // save out/err here and then optionally save the output if given
        self.stdout = Some(cfg.into());
        self
    }

    pub fn set_stdout<T: Into<PathBuf>>(&mut self, cfg: T) {
        // save out/err here and then optionally save the output if given
        self.stdout = Some(cfg.into());
    }

    pub fn timeout<T: Into<Duration>>(mut self, timeout: T) -> Self {
        self.timeout = Some(timeout.into());
        self
    }

    pub fn set_timeout<T: Into<Duration>>(&mut self, timeout: T) {
        self.timeout = Some(timeout.into());
    }

    pub async fn run_with<E: ProcessExecutor>(&self, executor: &E) -> Result<Output> {
        executor.run(self).await
    }
}

impl Display for Command {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut output = Vec::new();

        output.extend(
            self.envs
                .iter()
                .map(|(key, val)| format!("{}={}", key, val)),
        );

        output.push(self.program.clone());

        output.extend(self.args.iter().map(|arg| {
            match arg.split_once(" ") {
                Some(_) => format!("{:?}", arg), // if multi word string, give quotation marks
                None => format!("{}", arg),
            }
        }));

        if let Some(stdin) = &self.stdin {
            output.extend(["<".to_string(), format!("{}", stdin)].into_iter());
        }

        output.extend(
            [(&self.stdout, ">"), (&self.stderr, "2>")]
                .into_iter()
                .filter_map(|(path_option, pipe_char)| {
                    path_option
                        .as_ref()
                        .map(|path| format!("{} {}", pipe_char, path.display()))
                }),
        );

        write!(f, "{}", output.join(" "))
    }
}

/// StdinPipe represents the possible ways to pipe input to a command.
#[derive(Clone, Debug)]
pub enum StdinPipe {
    String(String),
    Path(PathBuf),
    File(Arc<Mutex<File>>),
}

impl Display for StdinPipe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self {
            Self::String(s) => write!(f, "{:?}", s),
            Self::Path(path) => write!(f, "{}", path.display()),
            Self::File(_file) => write!(f, "[file pointer]"),
        }
    }
}

/// The output from running a Command in an executor. The stdout/stderr of a command is always
/// captured, and optionally also written to a file if the stdout/stderr options were set on the
/// command. This is not as efficient as it could be, but does make things a bit easier at the cost
/// of memory efficiency.
///
/// ProcessExitStatus contains the exit code. Negative exit codes are wrapped around 256, so if a
/// program exits -10, then the resulting exit code will be 246
#[derive(Clone)]
pub struct Output {
    pub status: ExitStatus,
    pub stdout: String,
    pub stderr: String,
}

impl Output {
    pub fn new<O: AsRef<str>, E: AsRef<str>>(status: ExitStatus, stdout: O, stderr: E) -> Self {
        Self {
            status,
            stdout: stdout.as_ref().to_string(),
            stderr: stderr.as_ref().to_string(),
        }
    }

    pub fn from_exit_status(status: ExitStatus) -> Self {
        Self {
            status,
            stdout: "".to_string(),
            stderr: "".to_string(),
        }
    }
}

impl From<(StdExitStatus, Vec<u8>, Vec<u8>)> for Output {
    fn from((status, stdout, stderr): (StdExitStatus, Vec<u8>, Vec<u8>)) -> Self {
        Self {
            status: ExitStatus::from(status),
            stdout: String::from_utf8_lossy(&stdout).to_string(),
            stderr: String::from_utf8_lossy(&stderr).to_string(),
        }
    }
}

/// ProcessExecutor represents a way to run a command.
#[async_trait]
pub trait ProcessExecutor: Send + Sync + Clone {
    async fn run(&self, cmd: &Command) -> Result<Output>;
}

/// TokioExecutor will run a given command through the tokio::process::Command interface which will
/// result in creating a child process with the properties specified by Command.
#[derive(Clone)]
pub struct ShellExecutor;

impl ShellExecutor {
    fn attach_pipes(cmd: &Command, process: &mut TokioCommand) {
        if cmd.stdin.is_some() {
            process.stdin(Stdio::piped());
        }

        if cmd.stderr.is_some() {
            process.stdin(Stdio::piped());
        }

        if cmd.stdout.is_some() {
            process.stdout(Stdio::piped());
        }
    }

    fn spawn_stdin_task(stdin: StdinPipe, mut pipe: ChildStdin) -> JoinHandle<Result<()>> {
        let handle = tokio::spawn(async move {
            match &stdin {
                StdinPipe::String(string) => {
                    let mut reader = string.as_bytes();
                    copy(&mut reader, &mut pipe).await?;
                }
                StdinPipe::Path(path) => {
                    let mut reader = File::open(path).await?;
                    copy(&mut reader, &mut pipe).await?;
                }
                StdinPipe::File(file) => {
                    let mut file = &mut *file.lock().await;
                    // rewind the curser to the beginning of the file. This is to prevent any
                    // issued where a user writes to a file and then expects that write to show up
                    // when we pipe it to the process If we didn't rewind, then the cursor for the
                    // file would remain at the same place as when the write completed, which means
                    // no content would be piped.
                    file.rewind().await?;
                    copy(&mut file, &mut pipe).await?;
                }
            }
            Ok(())
        });

        handle
    }

    fn spawn_io(cmd: &Command, child: &mut Child) -> Result<ProcessIo> {
        let mut io = ProcessIo::default();

        if let Some(stdin) = &cmd.stdin {
            let pipe = child
                .stdin
                .take()
                .context("expected spawned child to have a stdin pipe")?;
            io.stdin = Some(Self::spawn_stdin_task(stdin.clone(), pipe));
        }

        let pipe = child
            .stdout
            .take()
            .context("expected spawned child to have stdout pipe")?;
        io.stdout = Some(tokio::spawn(async move {
            let mut reader = BufReader::new(pipe);
            let mut buffer = Vec::new();
            reader.read_to_end(&mut buffer).await?;
            Ok(buffer)
        }));

        let pipe = child
            .stderr
            .take()
            .context("expected spawned child to have stderr pipe")?;
        io.stderr = Some(tokio::spawn(async move {
            let mut reader = BufReader::new(pipe);
            let mut buffer = Vec::new();
            reader.read_to_end(&mut buffer).await?;
            Ok(buffer)
        }));

        Ok(io)
    }

    async fn write_results_to_file(
        cmd: &Command,
        stdout: &Option<Vec<u8>>,
        stderr: &Option<Vec<u8>>,
    ) -> Result<()> {
        join_all(
            [(cmd.stdout.clone(), stdout), (cmd.stderr.clone(), stderr)]
                .iter()
                .map(|(cmd, output)| async move {
                    if let Some(path) = cmd {
                        let output = output.as_ref().unwrap();
                        let mut file = File::create(path).await?;
                        file.write_all(output).await?;
                    }
                    Ok(())
                }),
        )
        .await
        .into_iter()
        .collect()
    }
}

#[derive(Default)]
struct ProcessIo {
    stdin: Option<JoinHandle<Result<()>>>,
    stdout: Option<JoinHandle<Result<Vec<u8>>>>,
    stderr: Option<JoinHandle<Result<Vec<u8>>>>,
}

impl ProcessIo {
    async fn join_all(self) -> Result<(Option<Vec<u8>>, Option<Vec<u8>>)> {
        if let Some(stdin) = self.stdin {
            stdin.await??;
        }

        let mut stdout = None;
        if let Some(pipe) = self.stdout {
            stdout = Some(pipe.await??);
        }

        let mut stderr = None;
        if let Some(pipe) = self.stderr {
            stderr = Some(pipe.await??);
        }

        Ok((stdout, stderr))
    }
}

#[async_trait]
impl ProcessExecutor for ShellExecutor {
    async fn run(&self, cmd: &Command) -> Result<Output> {
        info!("running {}", cmd);

        let mut process = TokioCommand::new(cmd.program.clone());
        process
            .kill_on_drop(true) // dropping the child results in sending sigkill to the process
            .args(cmd.args.clone())
            .env_clear() // the child process should have a fresh env
            .envs(cmd.envs.clone())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        if let Some(cwd) = &cmd.cwd {
            process.current_dir(cwd.clone());
        }

        Self::attach_pipes(cmd, &mut process);

        let mut child = process.spawn()?;

        let io = Self::spawn_io(cmd, &mut child)?;

        let res = match cmd.timeout {
            Some(duration) => {
                let res = timeout(duration, child.wait()).await;
                if let Err(_) = &res {
                    return Ok(Output::from_exit_status(ExitStatus::Timeout(duration)));
                }
                res.unwrap()
            }
            None => child.wait().await,
        };

        let status = res?;

        let (stdout, stderr) = io.join_all().await?;

        // if command had a stdout/err configured, then write that result to the file
        Self::write_results_to_file(cmd, &stdout, &stderr).await?;

        Ok((
            status,
            stdout.unwrap_or(Vec::new()),
            stderr.unwrap_or(Vec::new()),
        )
            .into())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExitStatus {
    Ok,
    Failure(i32),
    Timeout(Duration),
    Signal(SignalType),
}

impl ExitStatus {
    pub fn is_ok(&self) -> bool {
        match self {
            Self::Ok => true,
            _ => false,
        }
    }

    pub fn completed(&self) -> bool {
        match self {
            Self::Ok => true,
            Self::Failure(_) => true,
            _ => false,
        }
    }

    pub fn exit_code(&self) -> Option<i32> {
        match self {
            Self::Ok => Some(0),
            Self::Failure(rc) => Some(*rc),
            Self::Timeout(_) => None,
            Self::Signal(signal) => Some(signal.into()),
        }
    }
}

impl From<StdExitStatus> for ExitStatus {
    fn from(status: StdExitStatus) -> Self {
        if status.success() {
            return ExitStatus::Ok;
        }

        if let Some(rc) = status.code() {
            return ExitStatus::Failure(rc);
        }

        if let Some(signal) = status.signal() {
            return ExitStatus::Signal(signal.into());
        }

        unreachable!("exhausted std exit status options");
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SignalType {
    SegFault,
    Abort,
}

impl From<i32> for SignalType {
    fn from(value: i32) -> Self {
        match value {
            11 => Self::SegFault,
            6 => Self::Abort,
            _ => panic!("unexpected signal {}", value),
        }
    }
}

impl From<&SignalType> for i32 {
    fn from(value: &SignalType) -> Self {
        match *value {
            SignalType::SegFault => 11,
            SignalType::Abort => 6,
        }
    }
}

pub fn is_program_in_path(program: &str) -> bool {
    if let Ok(path) = env::var("PATH") {
        for p in path.split(":") {
            let p_str = format!("{}/{}", p, program);
            if fs::metadata(p_str).is_ok() {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use tempfile::{tempdir, TempDir};
    use tokio::{fs::OpenOptions, io::AsyncWriteExt};

    struct TempPathGuard<P> {
        _temp: P,
        pub path: PathBuf,
    }

    const TESTING_MAIN: &'static str = "resources/process/main.c";

    async fn create_temp_file_with_contents<N, C>(name: N, contents: C) -> TempPathGuard<TempDir>
    where
        N: AsRef<Path>,
        C: AsRef<[u8]>,
    {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(name);
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .truncate(true)
            .create(true)
            .open(&path)
            .await
            .unwrap();

        file.write_all(contents.as_ref()).await.unwrap();
        file.flush().await.unwrap();

        TempPathGuard {
            _temp: dir,
            path: path.to_path_buf(),
        }
    }

    async fn compile_and_get_testing_main() -> TempPathGuard<TempDir> {
        let mut testing_main = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        testing_main.push(TESTING_MAIN);
        assert!(
            testing_main.exists(),
            "Expected to find testing main at {}",
            TESTING_MAIN
        );

        let temp_dir = tempdir().unwrap();
        let temp_main = temp_dir.path().join("main.c");
        tokio::fs::copy(&testing_main, &temp_main).await.unwrap();

        let mut cmd = TokioCommand::new("gcc");
        cmd.args(["main.c", "-o", "test"]);
        cmd.current_dir(&temp_dir);
        let res = cmd.output().await.unwrap();
        assert!(res.status.success());

        let path = temp_dir.path().join("test");
        TempPathGuard {
            _temp: temp_dir,
            path,
        }
    }

    #[tokio::test]
    async fn captures_stdout() {
        let res = Command::new("echo")
            .arg("Hello there kenobi")
            .run_with(&ShellExecutor)
            .await
            .unwrap();

        assert_eq!(&res.stdout, "Hello there kenobi\n");
        assert_eq!(&res.stderr, "");
    }

    #[tokio::test]
    async fn captures_stderr() {
        let program = compile_and_get_testing_main().await;

        let res = Command::new(program.path.to_str().unwrap())
            .args(["stderr", "print this to stderr"])
            .run_with(&ShellExecutor)
            .await
            .unwrap();
        assert_eq!(&res.stdout, "");
        assert_eq!(&res.stderr, "print this to stderr\n");
    }

    #[tokio::test]
    async fn captures_stderr_stdout() {
        let program = compile_and_get_testing_main().await;
        let res = Command::new(program.path.to_str().unwrap())
            .args(["stdouterr", "yoda"])
            .run_with(&ShellExecutor)
            .await
            .unwrap();

        assert_eq!(&res.stdout, "OUT: yoda\n");
        assert_eq!(&res.stderr, "ERR: yoda\n");
    }

    #[tokio::test]
    async fn captures_nonzero_exit_code() {
        let program = compile_and_get_testing_main().await;
        let res = Command::new(program.path.to_str().unwrap())
            .args(["rc", "6"])
            .run_with(&ShellExecutor)
            .await
            .unwrap();

        assert_eq!(res.status, ExitStatus::Failure(6));

        let program = compile_and_get_testing_main().await;
        let res = Command::new(program.path.to_str().unwrap())
            .args(["rc", "-10"])
            .run_with(&ShellExecutor)
            .await
            .unwrap();

        assert_eq!(res.status, ExitStatus::Failure(246));
    }

    #[tokio::test]
    async fn catches_timeout() {
        let program = compile_and_get_testing_main().await;
        let res = Command::new(program.path.to_str().unwrap())
            .args(["timeout", "1"])
            .timeout(Duration::from_millis(1))
            .run_with(&ShellExecutor)
            .await
            .unwrap();

        assert!(matches!(res.status, ExitStatus::Timeout(_)));
    }

    #[tokio::test]
    async fn catches_segfault() {
        let program = compile_and_get_testing_main().await;
        let res = Command::new(program.path.to_str().unwrap())
            .args(["abort"])
            .run_with(&ShellExecutor)
            .await
            .unwrap();

        assert_eq!(res.status, ExitStatus::Signal(SignalType::Abort));
    }

    #[tokio::test]
    async fn read_stdin_from_open_file() {
        let program = compile_and_get_testing_main().await;
        let mut file = File::from_std(tempfile::tempfile().unwrap());
        file.write_all(b"file contents").await.unwrap();
        file.flush().await.unwrap();

        let res = Command::new(program.path.to_str().unwrap())
            .args(["read_line_from_stdin"])
            .stdin(StdinPipe::File(Arc::new(Mutex::new(file))))
            .run_with(&ShellExecutor)
            .await
            .unwrap();

        assert!(res.status.is_ok());
        assert_eq!(&res.stdout, "file contents");
    }

    #[tokio::test]
    async fn read_file_from_path() {
        let program = compile_and_get_testing_main().await;
        let file = create_temp_file_with_contents("name", "crazy stuff").await;
        let res = Command::new(program.path.to_str().unwrap())
            .args(["read_line_from_stdin"])
            .stdin(StdinPipe::Path(file.path.clone()))
            .run_with(&ShellExecutor)
            .await
            .unwrap();

        assert!(res.status.is_ok());
        assert_eq!(&res.stdout, "crazy stuff");
    }

    #[tokio::test]
    async fn read_stdin_from_string() {
        let program = compile_and_get_testing_main().await;
        let res = Command::new(program.path.to_str().unwrap())
            .args(["read_line_from_stdin"])
            .stdin(StdinPipe::String("read from stdin".to_string()))
            .run_with(&ShellExecutor)
            .await
            .unwrap();

        assert!(res.status.is_ok());
        assert_eq!(&res.stdout, "read from stdin");
    }

    #[tokio::test]
    async fn write_io_to_file() {
        let program = compile_and_get_testing_main().await;
        let stderr_file = create_temp_file_with_contents("stderr", "").await;
        let stdout_file = create_temp_file_with_contents("stdout", "").await;

        let res = Command::new(program.path.to_str().unwrap())
            .args(["stdouterr", "write me"])
            .stdout(stdout_file.path.clone())
            .stderr(stderr_file.path.clone())
            .run_with(&ShellExecutor)
            .await
            .unwrap();

        let mut contents = String::new();
        let mut file = File::open(stdout_file.path).await.unwrap();
        file.read_to_string(&mut contents).await.unwrap();
        assert_eq!(&contents, "OUT: write me\n");
        assert_eq!(&res.stdout, "OUT: write me\n");

        contents.clear();
        let mut file = File::open(stderr_file.path).await.unwrap();
        file.read_to_string(&mut contents).await.unwrap();
        assert_eq!(&contents, "ERR: write me\n");
        assert_eq!(&res.stderr, "ERR: write me\n");
    }
}
