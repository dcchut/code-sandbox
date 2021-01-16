FROM bitnami/minideb:latest as toolchain

RUN install_packages python3

# Set up a user
RUN useradd -m playground -d /playground
RUN usermod -p '!!' root # Disable all passwords for root
USER playground
ENV USER=playground
ENV PATH=/playground/.cargo/bin:$PATH
WORKDIR /playground

# Add our entry point
ADD --chown=playground entrypoint.sh /playground/tools/

ENTRYPOINT ["/playground/tools/entrypoint.sh"]