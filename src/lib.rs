use std::{fs, io, path::PathBuf, string, time::Duration};
use tempdir::TempDir;
use thiserror::Error;
use tokio::process::Command;

const DOCKER_PROCESS_TIMEOUT_SOFT: Duration = Duration::from_secs(4);
const DOCKER_PROCESS_TIMEOUT_HARD: Duration = Duration::from_secs(8);

#[derive(Error, Debug)]
pub enum Error {
    #[error("Unable to create temporary directory: {source}")]
    UnableToCreateTempDir { source: io::Error },
    #[error("Unable to create output directory: {source}")]
    UnableToCreateOutputDir { source: io::Error },
    #[error("Unable to set permissions for output directory: {source}")]
    UnableToSetOutputPermissions { source: io::Error },
    #[error("Unable to create source file: {source}")]
    UnableToCreateSourceFile { source: io::Error },
    #[error("Unable to set pemissions for source file: {source}")]
    UnableToSetSourcePermissions { source: io::Error },
    #[error("Unable to start the compiler: {source}")]
    UnableToStartCompiler { source: io::Error },
    #[error("Unable to find the compiler ID")]
    MissingCompilerId,
    #[error("Unable to wait for the compiler: {source}")]
    UnableToWaitForCompiler { source: io::Error },
    #[error("Unable to get output from the compiler: {source}")]
    UnableToGetOutputFromCompiler { source: io::Error },
    #[error("Unable to remove the compiler: {source}")]
    UnableToRemoveCompiler { source: io::Error },
    #[error("Compiler execution took longer than {} ms", timeout.as_millis())]
    CompilerExecutionTimedOut {
        source: tokio::time::error::Elapsed,
        timeout: Duration,
    },
    #[error("Unable to read output file: {source}")]
    UnableToReadOutput { source: io::Error },
    #[error("Unable to read crate information: {source}")]
    UnableToParseCrateInformation { source: ::serde_json::Error },
    #[error("Output was not valid UTF-8: {source}")]
    OutputNotUtf8 { source: string::FromUtf8Error },
    #[error("Output was missing")]
    OutputMissing,
    #[error("Release was missing from the version output")]
    VersionReleaseMissing,
    #[error("Commit hash was missing from the version output")]
    VersionHashMissing,
    #[error("Commit date was missing from the version output")]
    VersionDateMissing,
}

pub type Result<T, E = Error> = ::std::result::Result<T, E>;

pub struct Sandbox {
    #[allow(dead_code)]
    scratch: TempDir,
    input_file: PathBuf,
    output_dir: PathBuf,
}

// We must create a world-writable files (rustfmt) and directories
// (LLVM IR) so that the process inside the Docker container can write
// into it.
//
// This problem does *not* occur when using the indirection of
// docker-machine.
fn wide_open_permissions() -> Option<std::fs::Permissions> {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::PermissionsExt;
        Some(PermissionsExt::from_mode(0o777))
    }

    #[cfg(not(target_os = "linux"))]
    None
}

#[derive(Debug, Clone)]
pub enum Engine {
    Python,
    Rust,
}

impl Engine {
    fn docker_image(&self) -> &'static str {
        match self {
            Engine::Python => "code-sandbox-python",
            Engine::Rust => "code-sandbox-rust-stable",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ExecuteRequest {
    pub code: String,
    pub engine: Engine,
}

#[derive(Debug, Clone)]
pub struct ExecuteResponse {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

impl Sandbox {
    pub fn new() -> Result<Self> {
        let scratch =
            TempDir::new("playground").map_err(|e| Error::UnableToCreateTempDir { source: e })?;
        let input_file = scratch.path().join("input");
        let output_dir = scratch.path().join("output");

        fs::create_dir(&output_dir).map_err(|e| Error::UnableToCreateOutputDir { source: e })?;

        if let Some(perms) = wide_open_permissions() {
            fs::set_permissions(&output_dir, perms)
                .map_err(|e| Error::UnableToSetOutputPermissions { source: e })?;
        }

        Ok(Sandbox {
            scratch,
            input_file,
            output_dir,
        })
    }

    fn write_source_code(&self, code: &str) -> Result<()> {
        fs::write(&self.input_file, code)
            .map_err(|e| Error::UnableToCreateSourceFile { source: e })?;

        if let Some(perms) = wide_open_permissions() {
            fs::set_permissions(&self.input_file, perms)
                .map_err(|e| Error::UnableToSetSourcePermissions { source: e })?;
        }

        log::debug!(
            "Wrote {} bytes of source to {}",
            code.len(),
            self.input_file.display()
        );
        Ok(())
    }

    pub async fn execute(&self, req: &ExecuteRequest) -> Result<ExecuteResponse> {
        self.write_source_code(&req.code)?;
        let command = self.execute_command(req);

        let output = run_command_with_timeout(command).await?;

        Ok(ExecuteResponse {
            success: output.status.success(),
            stdout: vec_to_str(output.stdout)?,
            stderr: vec_to_str(output.stderr)?,
        })
    }

    fn execute_command(&self, req: &ExecuteRequest) -> Command {
        let mut cmd = self.docker_command(req);
        set_execution_environment(&mut cmd);
        let execution_cmd = build_execution_command(req);

        cmd.arg(req.engine.docker_image()).args(&execution_cmd);

        log::debug!("Execution command is {:?}", cmd);

        cmd
    }

    fn docker_command(&self, req: &ExecuteRequest) -> Command {
        let mut mount_input_file = self.input_file.as_os_str().to_os_string();
        mount_input_file.push(":");
        mount_input_file.push("/playground/");

        match req.engine {
            Engine::Python => {
                mount_input_file.push("src/main.py");
            }
            Engine::Rust => {
                mount_input_file.push("src/main.rs");
            }
        }

        let mut mount_output_dir = self.output_dir.as_os_str().to_os_string();
        mount_output_dir.push(":");
        mount_output_dir.push("/playground-result");

        let mut cmd = basic_secure_docker_command();

        cmd.arg("--volume")
            .arg(&mount_input_file)
            .arg("--volume")
            .arg(&mount_output_dir);

        cmd
    }
}

fn build_execution_command(req: &ExecuteRequest) -> Vec<&'static str> {
    let mut cmd = vec![];

    match req.engine {
        Engine::Python => {
            cmd.push("python3");
            cmd.push("/playground/src/main.py")
        }
        Engine::Rust => {
            cmd.push("cargo");
            cmd.push("run");
            // cmd.push("--release")
        }
    }

    cmd
}

macro_rules! docker_command {
    ($($arg:expr),* $(,)?) => ({
        let mut cmd = Command::new("docker");
        $( cmd.arg($arg); )*
        cmd
    });
}

fn basic_secure_docker_command() -> Command {
    let mut cmd = docker_command!(
        "run",
        "--detach",
        "--cap-drop=ALL",
        // Needed to allow overwriting the file
        "--cap-add=DAC_OVERRIDE",
        "--security-opt=no-new-privileges",
        "--workdir",
        "/playground",
        "--net",
        "none",
        "--memory",
        "256m",
        "--memory-swap",
        "256m",
        "--env",
        format!(
            "PLAYGROUND_TIMEOUT={}",
            DOCKER_PROCESS_TIMEOUT_SOFT.as_secs()
        ),
    );

    if cfg!(feature = "fork-bomb-prevention") {
        cmd.args(&["--pids-limit", "512"]);
    }

    cmd.kill_on_drop(true);

    cmd
}

async fn run_command_with_timeout(mut command: Command) -> Result<std::process::Output> {
    // use std::os::unix::process::ExitStatusExt;
    let timeout = DOCKER_PROCESS_TIMEOUT_HARD;

    let output = command
        .output()
        .await
        .map_err(|e| Error::UnableToStartCompiler { source: e })?;

    // Exit early, in case we don't have the container
    if !output.status.success() {
        return Ok(output);
    }

    let output = String::from_utf8_lossy(&output.stdout);
    let id = output
        .lines()
        .next()
        .ok_or(Error::MissingCompilerId)?
        .trim();

    // ----------

    let mut command = docker_command!("wait", id);

    let timed_out = match tokio::time::timeout(timeout, command.output()).await {
        Ok(Ok(o)) => {
            // Didn't time out, didn't fail to run
            let o = String::from_utf8_lossy(&o.stdout);

            #[cfg(target_os = "linux")]
            {
                use std::os::unix::process::ExitStatusExt;
                let code = o
                    .lines()
                    .next()
                    .unwrap_or("")
                    .trim()
                    .parse()
                    .unwrap_or(i32::MAX);
                Ok(ExitStatusExt::from_raw(code))
            }

            #[cfg(not(target_os = "linux"))]
            {
                use std::os::windows::process::ExitStatusExt;
                let code = o
                    .lines()
                    .next()
                    .unwrap_or("")
                    .trim()
                    .parse()
                    .unwrap_or(u32::MAX);
                Ok(ExitStatusExt::from_raw(code))
            }
        }
        Ok(e) => return e.map_err(|e| Error::UnableToWaitForCompiler { source: e }), // Failed to run
        Err(e) => Err(e),                                                            // Timed out
    };

    // ----------

    let mut command = docker_command!("logs", id);
    let mut output = command
        .output()
        .await
        .map_err(|e| Error::UnableToGetOutputFromCompiler { source: e })?;

    // ----------

    let mut command = docker_command!(
        "rm", // Kills container if still running
        "--force", id
    );
    command.stdout(std::process::Stdio::null());
    command
        .status()
        .await
        .map_err(|e| Error::UnableToRemoveCompiler { source: e })?;

    let code = timed_out.map_err(|e| Error::CompilerExecutionTimedOut { source: e, timeout })?;

    output.status = code;

    Ok(output)
}

fn vec_to_str(v: Vec<u8>) -> Result<String> {
    String::from_utf8(v).map_err(|e| Error::OutputNotUtf8 { source: e })
}

fn set_execution_environment(cmd: &mut Command) {
    cmd.args(&["--env", &format!("PLAYGROUND_EDITION={}", "2018")]);
}

#[cfg(test)]
mod test {
    use super::*;

    // Running the tests completely in parallel causes spurious
    // failures due to my resource-limited Docker
    // environment. Additionally, we have some tests that *require*
    // that no other Docker processes are running.
    fn one_test_at_a_time() -> impl Drop {
        use lazy_static::lazy_static;
        use std::sync::Mutex;

        lazy_static! {
            static ref DOCKER_SINGLETON: Mutex<()> = Default::default();
        }

        // We can't poison the empty tuple
        DOCKER_SINGLETON.lock().unwrap_or_else(|e| e.into_inner())
    }

    const HELLO_WORLD_CODE: &'static str = r#"
    fn main() {
        println!("Hello, world!");
    }
    "#;

    impl Default for ExecuteRequest {
        fn default() -> Self {
            ExecuteRequest {
                code: HELLO_WORLD_CODE.to_string(),
                engine: Engine::Rust,
            }
        }
    }

    #[tokio::test]
    async fn basic_functionality_rust() {
        let _singleton = one_test_at_a_time();
        let req = ExecuteRequest::default();

        let sb = Sandbox::new().expect("Unable to create sandbox");
        let resp = sb.execute(&req).await.expect("Unable to execute code");
        assert!(resp.stdout.contains("Hello, world!"));
    }

    #[tokio::test]
    async fn basic_functionality_python() {
        let _singleton = one_test_at_a_time();
        let req = ExecuteRequest {
            code: "print('hello world')".to_string(),
            engine: Engine::Python,
        };

        let sb = Sandbox::new().expect("Unable to create sandbox");
        let resp = sb.execute(&req).await.expect("Unable to execute code");
        assert!(resp.stdout.contains("hello world"));
    }

    #[tokio::test]
    async fn test_timeout_rust() {
        let _singleton = one_test_at_a_time();
        let req = ExecuteRequest {
            code: "fn main() { loop {} }".to_string(),
            engine: Engine::Rust,
        };

        let sb = Sandbox::new().expect("Unable to create sandbox");
        let resp = sb.execute(&req).await.expect("Unable to execute code");
        // Check we got a timeout message
        assert!(resp.stderr.contains("Killed"));
    }
}
