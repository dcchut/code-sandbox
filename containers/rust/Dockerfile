FROM bitnami/minideb:latest as toolchain

ARG channel

ENV DEBIAN_FRONTEND="noninteractive"

RUN apt-get update && \
    apt-get install ca-certificates -y && \
    update-ca-certificates && \
    install_packages curl build-essential

RUN useradd -m playground -d /playground
RUN usermod -p '!!' root # Disable all passwords for root
USER playground
ENV USER=playground
ENV PATH=/playground/.cargo/bin:$PATH
WORKDIR /playground

# Ensure that we are using the latest stable version of rustup and the
# latest version of the current channel. A new manifest will trigger
# these lines to run again, forcing a new download of rustup and
# installation of Rust.
ADD --chown=playground https://static.rust-lang.org/rustup/release-stable.toml /playground/tools/rustup-manifest.toml
ADD --chown=playground https://static.rust-lang.org/dist/channel-rust-${channel}-date.txt /playground/tools/rust-channel-version
RUN curl https://sh.rustup.rs -sSf | sh -s -- -y --profile minimal --default-toolchain "${channel}"
ADD --chown=playground entrypoint.sh /playground/tools/

# Fetch all the crate source files

FROM toolchain as bare-sources

RUN cargo init /playground

ADD --chown=playground Cargo.toml /playground/Cargo.toml
RUN cargo fetch

# Compiler and sources

FROM bare-sources as sources

ARG channel

RUN cargo build
RUN cargo build --release
RUN rm src/*.rs

ENTRYPOINT ["/playground/tools/entrypoint.sh"]