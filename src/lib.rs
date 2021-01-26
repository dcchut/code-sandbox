use std::io::Read;
use std::process::ExitStatus;
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
    #[error("Container path has no extension")]
    ContainerPathExtensionMissing,
}

pub type Result<T, E = Error> = ::std::result::Result<T, E>;

/// Represents some content that will be available inside the container.
#[derive(Debug)]
struct MountRequest {
    id: MountId,
    local_path: PathBuf,
    container_path: PathBuf,
    contents: String,
}

impl MountRequest {
    /// Performs the requested mount, storing `contents` in local path.
    fn mount(self) -> Result<Mount> {
        fs::write(&self.local_path, self.contents)
            .map_err(|e| Error::UnableToCreateSourceFile { source: e })?;

        Ok(Mount {
            id: self.id,
            local_path: self.local_path,
            container_path: self.container_path,
        })
    }
}

#[derive(Copy, Clone, Debug)]
pub struct MountId(usize);

#[derive(Debug)]
struct Mount {
    id: MountId,
    local_path: PathBuf,
    container_path: PathBuf,
}

impl Mount {
    fn to_command(&self) -> String {
        format!(
            "{}:{}",
            self.local_path.display(),
            self.container_path.display()
        )
    }
}

pub struct SandboxBuilder {
    /// The name of the docker image that will be used for this sandbox
    image: String,

    /// The entry point to call
    entry_point: Vec<&'static str>,

    /// Temporary directory where all the good stuff is happening
    scratch: TempDir,

    /// Folder which we contain any inputs that need to be mounted
    inputs: PathBuf,

    /// Keep track of the files we wish to mount
    mounts: Vec<MountRequest>,

    /// The length of the soft timeout
    soft_timeout: Option<Duration>,

    /// The length of the hard timeout
    hard_timeout: Option<Duration>,
}

#[derive(Debug)]
pub struct Sandbox {
    image: String,
    scratch: TempDir,
    inputs: PathBuf,
    mounts: Vec<Mount>,
    entry_point: Vec<&'static str>,
    soft_timeout: Duration,
    hard_timeout: Duration,
}

impl SandboxBuilder {
    /// Creates a new empty sandbox
    pub fn new<S: AsRef<str>>(image: S, entry_point: Vec<&'static str>) -> Result<Self> {
        let scratch =
            TempDir::new("playground").map_err(|e| Error::UnableToCreateTempDir { source: e })?;

        // Make a subdirectory to keep our mounts in.
        let inputs = scratch.path().join("inputs");
        fs::create_dir(&inputs).map_err(|e| Error::UnableToCreateTempDir { source: e })?;

        Ok(Self {
            image: String::from(image.as_ref()),
            entry_point,
            scratch,
            inputs,
            mounts: Vec::new(),
            soft_timeout: None,
            hard_timeout: None,
        })
    }

    /// Sets the soft and hard timeout for this container.
    ///
    /// # Panics
    /// This will panic if the soft timeout is longer than the hard timeout.
    pub fn with_timeout(&mut self, soft_timeout: Duration, hard_timeout: Duration) {
        assert!(soft_timeout <= hard_timeout);

        self.soft_timeout = Some(soft_timeout);
        self.hard_timeout = Some(hard_timeout);
    }

    /// Records a file that will be mounted in the container at a given path. The container
    /// path *must* have an extension.
    pub fn mount<P: Into<PathBuf>>(
        &mut self,
        container_path: P,
        contents: String,
    ) -> Result<MountId> {
        let container_path = container_path.into();

        let ext = container_path
            .extension()
            .ok_or(Error::ContainerPathExtensionMissing)?;
        let local_path =
            self.inputs
                .join(format!("{}.{}", self.mounts.len(), ext.to_string_lossy()));

        let id = MountId(self.mounts.len());

        self.mounts.push(MountRequest {
            id,
            local_path,
            container_path,
            contents,
        });

        Ok(id)
    }

    /// Finalizes the container, constructing a `CompleteSandbox` which can then be executed.
    /// After this point there is no way to mount additional files
    pub fn build(self) -> Result<Sandbox> {
        // Mount all of our requests
        let mounts = self
            .mounts
            .into_iter()
            .map(|request| request.mount())
            .collect::<Result<Vec<_>>>()?;

        Ok(Sandbox {
            image: self.image,
            scratch: self.scratch,
            inputs: self.inputs,
            mounts,
            entry_point: self.entry_point,
            soft_timeout: self.soft_timeout.unwrap_or(DOCKER_PROCESS_TIMEOUT_SOFT),
            hard_timeout: self.hard_timeout.unwrap_or(DOCKER_PROCESS_TIMEOUT_HARD),
        })
    }
}

impl Sandbox {
    /// Executes the current sandbox, returning the output from within the sandbox.
    pub async fn execute(self) -> Result<CompletedSandbox> {
        let cmd = self.execution_command();

        let output = run_command_with_timeout(cmd, self.hard_timeout).await?;

        Ok(CompletedSandbox::new(self.mounts, self.scratch, output))
    }

    /// Build a command to execute our entry point.
    fn execution_command(&self) -> Command {
        let mut cmd = self.docker_command();

        // Last argument to docker should be the image name,
        // which we then follow up with the internal entry point.
        cmd.arg(&self.image).args(&self.entry_point);

        log::debug!("Execution command is {:?}", cmd);

        cmd
    }

    /// Builds a basic command for invoking docker
    fn docker_command(&self) -> Command {
        let mut cmd = basic_secure_docker_command(self.soft_timeout);

        for mount in &self.mounts {
            cmd.arg("--volume");
            cmd.arg(mount.to_command());
        }

        cmd
    }
}

#[derive(Debug)]
pub struct CompletedSandbox {
    mounts: Vec<Mount>,
    scratch: TempDir,
    stdout: String,
    stderr: String,
    status: ExitStatus,
}

impl CompletedSandbox {
    pub(crate) fn new(mounts: Vec<Mount>, scratch: TempDir, output: std::process::Output) -> Self {
        // Process the output into something usable.
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let status = output.status;

        Self {
            mounts,
            scratch,
            stdout,
            stderr,
            status,
        }
    }

    /// Returns the contents of `stdout` from within the sandbox.
    pub fn stdout(&self) -> &str {
        self.stdout.as_str()
    }

    /// Return the contents of `stderr` from within the sandbox.
    pub fn stderr(&self) -> &str {
        self.stderr.as_str()
    }

    /// Return the exit status of the sandbox.
    pub fn status(&self) -> ExitStatus {
        self.status
    }

    /// Returns the contents of a mounted file.
    ///
    /// # Panics
    /// Will panic is `id` does not correspond to a mount for this sandbox.
    pub fn get_mount_output(&self, id: MountId) -> Result<String> {
        let mount = self.mounts.get(id.0).expect("invalid mount id");

        // Load the input file
        let mut contents = String::new();
        // TODO: fix this error type up
        let mut file = std::fs::File::open(&mount.local_path)
            .map_err(|_| Error::ContainerPathExtensionMissing)?;
        file.read_to_string(&mut contents)
            .map_err(|_| Error::ContainerPathExtensionMissing)?;

        Ok(contents)
    }
}

macro_rules! docker_command {
    ($($arg:expr),* $(,)?) => ({
        let mut cmd = Command::new("docker");
        $( cmd.arg($arg); )*
        cmd
    });
}

fn basic_secure_docker_command(soft_timeout: Duration) -> Command {
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
        "512m",
        "--memory-swap",
        "512m",
        "--env",
        format!(
            "PLAYGROUND_TIMEOUT={}",
            soft_timeout.as_secs()
        ),
    );

    if cfg!(feature = "fork-bomb-prevention") {
        cmd.args(&["--pids-limit", "512"]);
    }

    cmd.kill_on_drop(true);

    cmd
}

async fn run_command_with_timeout(mut command: Command, hard_timeout: Duration) -> Result<std::process::Output> {
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

    let timed_out = match tokio::time::timeout(hard_timeout, command.output()).await {
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

    let code = timed_out.map_err(|e| Error::CompilerExecutionTimedOut { source: e, timeout: hard_timeout })?;

    output.status = code;

    Ok(output)
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

    async fn run_rust_sandbox<S: AsRef<str>>(code: S) -> CompletedSandbox {
        let mut builder =
            SandboxBuilder::new("dcchut/code-sandbox-rust-stable", vec!["cargo", "run"])
                .expect("failed to build sandbox");
        builder
            .mount("/playground/src/main.rs", code.as_ref().to_string())
            .expect("failed to mount code");

        let sb = builder.build().expect("failed to build sandbox");
        sb.execute().await.expect("failed to run sandbox")
    }

    async fn run_python_sandbox<S: AsRef<str>>(code: S) -> CompletedSandbox {
        let mut builder = SandboxBuilder::new(
            "dcchut/code-sandbox-python",
            vec!["python3", "/playground/src/main.py"],
        )
        .expect("failed to build sandbox");
        builder
            .mount("/playground/src/main.py", code.as_ref().to_string())
            .expect("failed to mount code");

        let sb = builder.build().expect("failed to build sandbox");
        sb.execute().await.expect("failed to run sandbox")
    }

    async fn run_haskell_sandbox<S: AsRef<str>>(code: S) -> CompletedSandbox {
        let mut builder = SandboxBuilder::new(
            "dcchut/code-sandbox-haskell",
            vec!["cabal", "run", "-v0"]
        ).expect("falied to build sandbox");

        builder.mount("/playground/Main.hs", code.as_ref().to_string())
            .expect("failed to mount code");

        let sb = builder.build().expect("failed to build sandbox");
        sb.execute().await.expect("failed to run sandbox")
    }

    #[tokio::test]
    async fn basic_functionality_rust() {
        let _singleton = one_test_at_a_time();
        let response = run_rust_sandbox(HELLO_WORLD_CODE).await;

        assert!(response.stdout.contains("Hello, world!"));
    }

    #[tokio::test]
    async fn basic_functionality_python() {
        let _singleton = one_test_at_a_time();
        let response = run_python_sandbox("print('hello world')").await;

        assert!(response.stdout.contains("hello world"));
    }

    #[tokio::test]
    async fn basic_functionality_haskell() {
        let _singleton = one_test_at_a_time();
        let response = run_haskell_sandbox(r#"
        module Main where

        main :: IO ()
        main = do
            print "hello world!"

        "#).await;

        // Note that haskell outputs the quotes around our string
        assert!(response.stdout.contains(r#""hello world!""#));
    }

    #[tokio::test]
    async fn test_timeout_rust() {
        let _singleton = one_test_at_a_time();
        let response = run_rust_sandbox("fn main() { loop{} }").await;

        // should get a failing status code
        assert!(!response.status.success());
        assert!(response.stderr.contains("Killed"));
    }

    /// Tests the full end-to-end process, including file mounting, execution, and solution retrieval.,
    #[tokio::test]
    async fn test_sandbox_end_to_end() {
        let mut builder =
            SandboxBuilder::new("dcchut/code-sandbox-rust-stable", vec!["cargo", "run"])
                .expect("failed to create builder");

        builder
            .mount(
                "/playground/src/solution.rs",
                String::from(
                    r#"
                pub fn hello_world() {
                    println!("hello world!");
                }
            "#,
                ),
            )
            .expect("failed to mount code");

        builder
            .mount(
                "/playground/src/main.rs",
                String::from(
                    r#"
                use std::fs::File;
                use std::io::prelude::*;

                mod solution;

                fn main() -> std::io::Result<()> {
                    solution::hello_world();

                    // Write some stuff to /output.txt
                    let mut file = File::create("output.txt")?;
                    file.write_all(b"asdasd")?;
                    Ok(())
                }
            "#,
                ),
            )
            .expect("failed to mount entry point");

        let output_mount_id = builder
            .mount("/playground/output.txt", String::new())
            .expect("failed to mount output");

        let sandbox = builder.build().expect("failed to build sandbox");

        let result = sandbox.execute().await.expect("failed to execute sandbox");

        // Process should have successfully completed.
        assert!(result.status.success());

        // Get the output from our outputs mount
        let outputs = result
            .get_mount_output(output_mount_id)
            .expect("failed to get mount output");

        assert_eq!(outputs, "asdasd");
    }

    /// Tests that the rust container has serde support.
    #[tokio::test]
    async fn test_serde_support() {
        let response = run_rust_sandbox(
            r#"
use serde_json::{Result, Value};

fn main() {
    println!("success");
}"#,
        )
        .await;

        assert!(response.status.success());
        assert!(response.stdout.contains("success"));
    }

    #[tokio::test]
    async fn basic_functionality_hassssskell() {
        let _singleton = one_test_at_a_time();
        let response = run_haskell_sandbox(r#"
        module Main where

        main :: IO ()
        main = do
            print "hello world!"

        "#).await;

        dbg!(&response);

        // Note that haskell outputs the quotes around our string
        assert!(response.stdout.contains(r#""hello world!""#));
    }


}
