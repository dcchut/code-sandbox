# code-sandbox

A proof-of-concept library for creating and running temporary Docker containers for the purpose of running code.

Some example use cases include:
- A backend component of a code editor and submission system, or
- To allow users of a Discord bot to run code.

## Docker images

This repository contains a few Dockerfiles that can be used to create images intended
for use in a code sandbox.  The provided images are:

| Image                             | Environment description |
|-----------------------------------|-------------------------|
| `dcchut/code-sandbox-rust-stable` | Latest stable Rust      |
| `dcchut/code-sandbox-python`      | Python 3.7              |
| `dcchut/code-sandbox-haskell`     | GHC 8.10.7              |

### Soft timeout wrapper

Each image has a basic wrapper script responsible for enforcing a soft timeout:

```shell
#!/bin/bash

set -eu

timeout=${PLAYGROUND_TIMEOUT:-10}

# Don't use `exec` here. The shell is what prints out the useful
# "Killed" message
timeout --signal=KILL ${timeout} "$@"
```

A hard timeout is enforced at the library level while the sandbox is being run.

### Creating a new image

Creating a new sandbox image is fairly straightforward: prepare a Dockerfile with
the desired environment whose entry point is the wrapper script (see above).  
Please feel free to submit a PR if you have a new image to contribute or improvements
to an existing image.

*Tip*: Make sure to make use of the Docker cache to reduce compilation time
inside the image!  For an example of this see `dcchut/code-sandbox-rust` -
build requirements are pre-compiled so the image only has to pay the compilation
cost of the code the user actually writes.

## `code-sandbox` crate

As mentioned above, this library is geared towards running short-lived Docker containers
for the purposes of running code - it is not a general purpose library for interacting
with Docker.

### Running your first sandbox

The following example shows mounting the `dcchut/code-sandbox-python` image and
running some Python code in it:

```rust
use code_sandbox::{Result, SandboxBuilder};

// Bog-standard iterative fibonacci implementation
const FIBONACCI: &'static str = r#"
def fib(n: int) -> int:
    """
    Defined by the relations:

    fib(n) = 1 if n <= 1
    fib(n+2) = fib(n+1) + fib(n) for n >= 0
    """
    p1, p2 = 1, 1
    for _ in range(n-1):
        p1, p2 = p1 + p2, p1
    return p1

print(fib(6))
"#;

#[tokio::main]
async fn main() -> Result<()> {
    let mut builder = SandboxBuilder::new("dcchut/code-sandbox-python")?;

    // Request the builder to mount our Fibonacci script when we run the image
    builder.mount("/app/main.py", FIBONACCI)?;

    // Set the entry point to run inside the image
    builder.entry_point(["python3", "/app/main.py"]);

    // Build and run the sandbox
    let result = builder.build()?.execute().await?;
    assert_eq!(result.stdout(), "13\n");

    Ok(())
}
```

### Reading the value of a mounted file.

It's also possible to mount a file within the sandbox, run the sandbox, then read the value of the mounted file
once the sandbox has been run.

```rust
use code_sandbox::{Result, SandboxBuilder};

// Reads in a file, modifies its contents, then writes the file back out again.
const SANDBOX_CODE: &'static str = r#"
with open("/app/contents.txt", "rt") as fp:
    contents = fp.read()

contents += "\nModified by the sandbox"

with open("/app/contents.txt", "wt") as fp:
    fp.write(contents)
"#;

#[tokio::main]
async fn main() -> Result<()> {
    // As in the previous example
    let mut builder = SandboxBuilder::new("dcchut/code-sandbox-python")?;
    builder.mount("/app/main.py", SANDBOX_CODE)?;
    builder.entry_point(["python3", "/app/main.py"]);

    // Request to mount a file at /app/contents.txt.  The mount method
    // returns an identifier which will be used to identify this file later.
    let contents_txt = builder.mount("/app/contents.txt", "Raspberry")?;

    // Build and run the sandbox
    let result = builder.build()?.execute().await?;

    // Get the contents of the mounted file
    let contents = result.get_mount_output(contents_txt)?;
    assert_eq!(contents, "Raspberry\nModified by the sandbox");

    Ok(())
}
```
