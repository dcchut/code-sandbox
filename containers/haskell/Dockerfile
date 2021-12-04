FROM bitnami/minideb:latest as toolchain

RUN apt-get update && \
    apt-get install ca-certificates libffi-dev libffi6 libgmp-dev libgmp10 libncurses-dev libncurses5 libtinfo5 -y && \
    update-ca-certificates && \
    install_packages curl build-essential

# Set up a user
RUN useradd -m playground -d /playground
RUN usermod -p '!!' root # Disable all passwords for root
USER playground
ENV USER=playground
ENV PATH=/playground/.ghcup/bin:$PATH
WORKDIR /playground

ENV BOOTSTRAP_HASKELL_NONINTERACTIVE=true
RUN curl --proto '=https' --tlsv1.2 -sSf https://get-ghcup.haskell.org | sh
# Add our entry point
ADD --chown=playground entrypoint.sh /playground/tools/

FROM toolchain as bare-sources

RUN cabal init --package-name dccode
ADD --chown=playground dccode.cabal /playground/dccode.cabal
RUN cabal build

# Compiler and sources
FROM bare-sources as sources
RUN rm /playground/app/Main.hs

ENTRYPOINT ["/playground/tools/entrypoint.sh"]