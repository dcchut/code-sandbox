//! # code-sandbox
//!
//! A proof-of-concept library for creating temporary Docker containers for the purposes of running code.
//! Some example use cases:
//! - A backend component of a code editor or submission system, or
//! - To allow users of a Discord bot to run code.
//!
//! ## Docker images
//!
//! This repository contains a few Dockerfiles that are intended for use in a code sandbox.
//! The provided images are:
//!
//! | Image                             | Environment description |
//! |-----------------------------------|-------------------------|
//! | `dcchut/code-sandbox-rust-stable` | Latest stable Rust      |
//! | `dcchut/code-sandbox-python`      | Python 3.7              |
//! | `dcchut/code-sandbox-haskell`     | GHC 8.10.7              |
//!
//! ### Soft timeout wrapper
//! Each image has a basic wrapper script responsible for enforcing a soft timeout:
//!
//! ```shell
//! #!/bin/bash
//!
//! set -eu
//!
//! timeout=${PLAYGROUND_TIMEOUT:-10}
//!
//! # Don't use `exec` here. The shell is what prints out the useful
//! # "Killed" message
//! timeout --signal=KILL ${timeout} "$@"
//! ```
//!
//! A hard timeout is enforced at the library level when the sandbox is being ran.
//!
//! ### Creating a new image
//!
//! Creating a new sandbox image is fairly straightforward: prepare a Dockerfile
//! with the desired environment whose entry point is the wrapper script (seen above).
//! Please feel free to put up a PR if you have a code sandbox to contribute or an
//! update or fix to an existing sandbox!
//!
//! *Tip*: Make sure to make use of the Docker cache to reduce compilation time
//! inside the image!  For an example of this see the `dcchut/code-sandbox-rust` Dockerfile -
//! build requirements are pre-compiled so that the image only has to pay the compilation
//! cost of the code the user actually writes.
//!
//! ## `code-sandbox` crate
//!
//! This library is geared towards running short-lived Docker containers
//! for the purposes of running code - it is not a general purpose library
//! for interacting with Docker.
//!
//! ### Running your first sandbox
//! The following example shows mounting the `dcchut/code-sandbox-python` image
//! and running some Python code in it:
//!
//! ```rust
//! use code_sandbox::{Result, SandboxBuilder};
//!
//! // Bog-standard iterative fibonacci implementation
//! const FIBONACCI: &'static str = r#"
//! def fib(n: int) -> int:
//!     """
//!     Defined by the relations:
//!
//!     fib(n) = 1 if n <= 1
//!     fib(n+2) = fib(n+1) + fib(n) for n >= 0
//!     """
//!     p1, p2 = 1, 1
//!     for _ in range(n-1):
//!         p1, p2 = p1 + p2, p1
//!     return p1
//!
//! print(fib(6))
//! "#;
//!
//! #[tokio::main]
//! async fn main() -> Result<()> {
//!     let mut builder = SandboxBuilder::new("dcchut/code-sandbox-python")?;
//!
//!     // Request the builder to mount our Fibonacci script when we run the image
//!     builder.mount("/app/main.py", FIBONACCI)?;
//!
//!     // Set the entry point to run inside the image
//!     builder.entry_point(["python3", "/app/main.py"]);
//!
//!     // Build and run the sandbox
//!     let result = builder.build()?.execute().await?;
//!     assert_eq!(result.stdout(), "13\n");
//!
//!     Ok(())
//! }
//! ```
//!
//! ### Reading the value of a mounted file.
//! It is also possible to read the contents of a mounted file after the sandbox
//! has been ran.
//!
//! ```rust
//! use code_sandbox::{Result, SandboxBuilder};
//!
//! // Reads in a file, modifies its contents, then writes the file back out again.
//! const SANDBOX_CODE: &'static str = r#"
//! with open("/app/contents.txt", "rt") as fp:
//!     contents = fp.read()
//!
//! contents += "\nModified by the sandbox"
//!
//! with open("/app/contents.txt", "wt") as fp:
//!     fp.write(contents)
//! "#;
//!
//! #[tokio::main]
//! async fn main() -> Result<()> {
//!     // As in the previous example
//!     let mut builder = SandboxBuilder::new("dcchut/code-sandbox-python")?;
//!     builder.mount("/app/main.py", SANDBOX_CODE)?;
//!     builder.entry_point(["python3", "/app/main.py"]);
//!
//!     // Request to mount a file at /app/contents.txt.  The mount method
//!     // returns an identifier which will be used to identify this file later.
//!     let contents_txt = builder.mount("/app/contents.txt", "Raspberry")?;
//!
//!     // Build and run the sandbox
//!     let result = builder.build()?.execute().await?;
//!
//!     // Get the contents of the mounted file
//!     let contents = result.get_mount_output(contents_txt)?;
//!     assert_eq!(contents, "Raspberry\nModified by the sandbox");
//!
//!     Ok(())
//! }
//! ```
//!
//!
use log::debug;
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
            local_path: self.local_path,
            container_path: self.container_path,
        })
    }
}

/// Used to identify a file that has been mounted in a container.
///
/// # Example
/// ```rust
/// use code_sandbox::{Result, SandboxBuilder};
///
/// #[tokio::main]
/// async fn main() -> Result<()> {
///     // Set up a sandbox which overwrites the contents of /app/mounted.txt
///     let mut builder = SandboxBuilder::new("dcchut/code-sandbox-python")?;
///     builder.entry_point(["python3", "/app/main.py"]);
///     builder.mount(
///         "/app/main.py",
///         r#"
/// with open("/app/mounted.txt", "wt") as fp:
///     fp.write("Hello World!")
/// "#,
///     )?;
///
///     // A [`MountId`] is returned when we request a mount.
///     let mount_id = builder.mount("/app/mounted.txt", "")?;
///     let result = builder.build()?.execute().await?;
///
///     // The [`MountID`] can then be used to obtain the value of the modified file.
///     assert_eq!(result.get_mount_output(mount_id)?, "Hello World!");
///
///     Ok(())
/// }
/// ```
#[derive(Copy, Clone, Debug)]
pub struct MountId(usize);

#[derive(Debug)]
struct Mount {
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

/// Builder struct to construct a [`Sandbox`].
pub struct SandboxBuilder {
    /// The name of the docker image that will be used for this sandbox
    image: String,

    /// The entry point to call
    entry_point: Vec<String>,

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
    _inputs: PathBuf,
    mounts: Vec<Mount>,
    entry_point: Vec<String>,
    soft_timeout: Duration,
    hard_timeout: Duration,
}

impl SandboxBuilder {
    /// Creates a new empty sandbox
    ///
    /// # Example
    /// ```rust
    /// use code_sandbox::{Result, SandboxBuilder};
    ///
    /// fn main() -> Result<()> {
    ///     let mut builder = SandboxBuilder::new("dcchut/code-sandbox-python")?;
    ///     let sandbox = builder.build()?;
    ///
    ///     Ok(())
    /// }
    /// ```
    pub fn new<S: AsRef<str>>(image: S) -> Result<Self> {
        let scratch =
            TempDir::new("playground").map_err(|e| Error::UnableToCreateTempDir { source: e })?;

        // Make a subdirectory to keep our mounts in.
        let inputs = scratch.path().join("inputs");
        fs::create_dir(&inputs).map_err(|e| Error::UnableToCreateTempDir { source: e })?;

        Ok(Self {
            image: String::from(image.as_ref()),
            entry_point: Vec::new(),
            scratch,
            inputs,
            mounts: Vec::new(),
            soft_timeout: None,
            hard_timeout: None,
        })
    }

    /// Set the entry point for this container.
    ///
    /// # Example
    /// ```rust
    /// use code_sandbox::{Result, SandboxBuilder};
    ///
    /// fn main() -> Result<()> {
    ///     let mut builder = SandboxBuilder::new("dcchut/code-sandbox-python")?;
    ///     builder.entry_point(["python3.6", "/app/main.py"]);
    ///
    ///     Ok(())
    /// }
    /// ```
    pub fn entry_point<I: IntoIterator<Item = R>, R: ToString>(
        &mut self,
        entry_point: I,
    ) -> &mut Self {
        self.entry_point = entry_point.into_iter().map(|r| r.to_string()).collect();
        self
    }

    /// Sets the soft and hard timeout for this container.
    ///
    /// # Panics
    /// This will panic if the soft timeout is longer than the hard timeout.
    ///
    /// # Example
    /// ```rust
    /// use code_sandbox::{Result, SandboxBuilder};
    /// use std::time::Duration;
    ///
    /// fn main() -> Result<()> {
    ///     let mut builder = SandboxBuilder::new("dcchut/code-sandbox-python")?;
    ///
    ///     // Soft timeout of 5 seconds, hard timeout of 10 seconds
    ///     builder.with_timeout(Duration::from_secs(5), Duration::from_secs(10));
    ///     Ok(())
    /// }
    /// ```
    pub fn with_timeout(&mut self, soft_timeout: Duration, hard_timeout: Duration) -> &mut Self {
        assert!(soft_timeout <= hard_timeout);

        self.soft_timeout = Some(soft_timeout);
        self.hard_timeout = Some(hard_timeout);

        self
    }

    /// Mounts `contents` at the given path within the container.
    ///
    /// # Example
    /// ```rust
    /// use code_sandbox::{Result, SandboxBuilder};
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<()> {
    ///     let mut builder = SandboxBuilder::new("alpine:3.15")?;
    ///     builder.entry_point(["/bin/sh", "-c", "echo 'final contents' > /app/mounted.txt"]);
    ///
    ///     let id = builder.mount("/app/mounted.txt", "initial contents")?;
    ///
    ///     // Mounted file was changed inside the sandbox
    ///     let result = builder.build()?.execute().await?;
    ///     assert_eq!(result.get_mount_output(id)?, "final contents\n");
    ///
    ///     Ok(())
    /// }
    /// ```
    pub fn mount<S: ToString, P: Into<PathBuf>>(
        &mut self,
        container_path: P,
        contents: S,
    ) -> Result<MountId> {
        let container_path = container_path.into();

        let local_path = self.inputs.join(format!("{}.txt", self.mounts.len()));

        let id = MountId(self.mounts.len());

        self.mounts.push(MountRequest {
            local_path,
            container_path,
            contents: contents.to_string(),
        });

        Ok(id)
    }

    /// Finalizes the container, constructing a [`Sandbox`] which can be executed.  After this
    /// point there is no way to mount additional files or change any configuration options.
    ///
    /// # Example
    /// ```rust
    /// use code_sandbox::{Result, SandboxBuilder};
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<()> {
    ///     let mut builder = SandboxBuilder::new("dcchut/code-sandbox-python")?;
    ///     builder.entry_point(["python3", "--version"]);
    ///
    ///     // Build the sandbox, ready for execution.
    ///     let sandbox = builder.build()?;
    ///
    ///     let result = sandbox.execute().await?;
    ///     assert_eq!(result.stdout().contains("Python 3."), true);
    ///
    ///     Ok(())
    /// }
    /// ```
    pub fn build(self) -> Result<Sandbox> {
        let mounts = self
            .mounts
            .into_iter()
            .map(|request| request.mount())
            .collect::<Result<Vec<_>>>()?;

        Ok(Sandbox {
            image: self.image,
            scratch: self.scratch,
            _inputs: self.inputs,
            mounts,
            entry_point: self.entry_point,
            soft_timeout: self.soft_timeout.unwrap_or(DOCKER_PROCESS_TIMEOUT_SOFT),
            hard_timeout: self.hard_timeout.unwrap_or(DOCKER_PROCESS_TIMEOUT_HARD),
        })
    }
}

impl Sandbox {
    /// Executes the current sandbox, returning the output from within the sandbox.
    ///
    /// # Example
    /// ```rust
    /// use code_sandbox::{Result, SandboxBuilder};
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<()> {
    ///     let mut builder = SandboxBuilder::new("dcchut/code-sandbox-rust-stable")?;
    ///     builder.entry_point(["cargo", "--version"]);
    ///
    ///     let result = builder.build()?.execute().await?;
    ///     Ok(())
    /// }
    /// ```
    pub async fn execute(self) -> Result<CompletedSandbox> {
        debug!("executing sandbox");
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
    stdout: String,
    stderr: String,
    status: ExitStatus,
    _scratch: TempDir,
}

impl CompletedSandbox {
    pub(crate) fn new(mounts: Vec<Mount>, scratch: TempDir, output: std::process::Output) -> Self {
        // Process the output into something usable.
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let status = output.status;

        Self {
            mounts,
            _scratch: scratch,
            stdout,
            stderr,
            status,
        }
    }

    /// Returns the contents of `stdout` from within the sandbox.
    ///
    /// # Example
    /// ```rust
    /// use code_sandbox::{Result, SandboxBuilder};
    ///
    /// const PYTHON_CODE: &'static str = r#"
    /// print("this ends up on stdout")
    /// "#;
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<()> {
    ///     let mut builder = SandboxBuilder::new("dcchut/code-sandbox-python")?;
    ///     builder.entry_point(["python3", "/app/main.py"]);
    ///     builder.mount("/app/main.py", PYTHON_CODE)?;
    ///
    ///     let result = builder.build()?.execute().await?;
    ///     assert_eq!(result.stdout(), "this ends up on stdout\n");
    ///
    ///     Ok(())
    /// }
    /// ```
    pub fn stdout(&self) -> &str {
        self.stdout.as_str()
    }

    /// Return the contents of `stderr` from within the sandbox.
    ///
    /// # Example
    /// ```rust
    /// use code_sandbox::{Result, SandboxBuilder};
    ///
    /// const PYTHON_CODE: &'static str = r#"
    /// import sys
    ///
    /// print("this ends up on stderr", file=sys.stderr)
    /// "#;
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<()> {
    ///     let mut builder = SandboxBuilder::new("dcchut/code-sandbox-python")?;
    ///     builder.entry_point(["python3", "/app/main.py"]);
    ///     builder.mount("/app/main.py", PYTHON_CODE)?;
    ///
    ///     let result = builder.build()?.execute().await?;
    ///     assert_eq!(result.stderr(), "this ends up on stderr\n");
    ///
    ///     Ok(())
    /// }
    /// ```
    pub fn stderr(&self) -> &str {
        self.stderr.as_str()
    }

    /// Return the exit status of the sandbox.
    ///
    /// # Example
    /// ```rust
    /// use code_sandbox::{Result, SandboxBuilder};
    ///
    /// const PYTHON_CODE: &'static str = r#"
    /// import sys
    /// sys.exit(17)
    /// "#;
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<()> {
    ///     let mut builder = SandboxBuilder::new("dcchut/code-sandbox-python")?;
    ///     builder.entry_point(["python3", "/app/main.py"]);
    ///     builder.mount("/app/main.py", PYTHON_CODE)?;
    ///
    ///     let result = builder.build()?.execute().await?;
    ///     assert_eq!(result.status().code().unwrap(), 17);
    ///
    ///     Ok(())
    /// }
    /// ```
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
        format!("PLAYGROUND_TIMEOUT={}", soft_timeout.as_secs()),
    );

    cmd.kill_on_drop(true);
    cmd
}

async fn run_command_with_timeout(
    mut command: Command,
    hard_timeout: Duration,
) -> Result<std::process::Output> {
    let output = command
        .output()
        .await
        .map_err(|e| Error::UnableToStartCompiler { source: e })?;

    debug!("{:?}", output);

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

    // Run the command for at most `hard_timeout`
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
        Ok(e) => return e.map_err(|e| Error::UnableToWaitForCompiler { source: e }),
        Err(e) => Err(e),
    };

    // Obtain the output (roughly, stdout and stderr) of the container.
    let mut command = docker_command!("logs", id);
    let mut output = command
        .output()
        .await
        .map_err(|e| Error::UnableToGetOutputFromCompiler { source: e })?;

    // Kill and remove the container if it's still running
    let mut command = docker_command!("rm", "--force", id);
    command.stdout(std::process::Stdio::null());
    command
        .status()
        .await
        .map_err(|e| Error::UnableToRemoveCompiler { source: e })?;

    // Get the status code of the command running in the container
    let status_code = timed_out.map_err(|e| Error::CompilerExecutionTimedOut {
        source: e,
        timeout: hard_timeout,
    })?;
    output.status = status_code;

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
        let mut builder = SandboxBuilder::new("dcchut/code-sandbox-rust-stable")
            .expect("failed to build sandbox");
        builder.entry_point(["cargo", "run"]);

        builder
            .mount("/playground/src/main.rs", code.as_ref().to_string())
            .expect("failed to mount code");

        let sb = builder.build().expect("failed to build sandbox");
        sb.execute().await.expect("failed to run sandbox")
    }

    async fn run_python_sandbox<S: AsRef<str>>(code: S) -> CompletedSandbox {
        let mut builder =
            SandboxBuilder::new("dcchut/code-sandbox-python").expect("failed to build sandbox");
        builder.entry_point(["python3", "/playground/src/main.py"]);

        builder
            .mount("/playground/src/main.py", code.as_ref().to_string())
            .expect("failed to mount code");

        let sb = builder.build().expect("failed to build sandbox");
        sb.execute().await.expect("failed to run sandbox")
    }

    async fn run_haskell_sandbox<S: AsRef<str>>(code: S) -> CompletedSandbox {
        let mut builder =
            SandboxBuilder::new("dcchut/code-sandbox-haskell").expect("falied to build sandbox");
        builder.entry_point(["cabal", "run", "-v0"]);

        builder
            .mount("/playground/app/Main.hs", code.as_ref().to_string())
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
        let response = run_haskell_sandbox(
            r#"
        module Main where

        main :: IO ()
        main = do
            print "hello world!"

        "#,
        )
        .await;

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
        let mut builder = SandboxBuilder::new("dcchut/code-sandbox-rust-stable")
            .expect("failed to create builder");
        builder.entry_point(["cargo", "run"]);

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
        let response = run_haskell_sandbox(
            r#"
        module Main where

        main :: IO ()
        main = do
            print "hello world!"

        "#,
        )
        .await;

        dbg!(&response);

        // Note that haskell outputs the quotes around our string
        assert!(response.stdout.contains(r#""hello world!""#));
    }
}
