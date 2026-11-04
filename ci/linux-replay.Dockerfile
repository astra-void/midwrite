# Linux test environment for Midwrite replay (Debian bookworm, Node 22).
#
# Used by scripts/linux-replay-docker.sh to run the full test suite,
# including required replay tests with bubblewrap isolation, on a Linux
# kernel from a macOS or Linux workstation. CI runners use the workflow in
# .github/workflows/ci.yml instead of this image.
#
# Containers need `--security-opt seccomp=unconfined --security-opt
# systempaths=unconfined` for bubblewrap: Docker's default seccomp profile
# forbids creating user namespaces and its masked /proc forbids mounting a
# fresh procfs. The container itself stays unprivileged (no added
# capabilities); the tests run as a non-root user.
FROM node:22-bookworm

RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      bubblewrap postgresql postgresql-contrib ca-certificates curl unzip git build-essential python3 \
 && rm -rf /var/lib/apt/lists/*

RUN useradd --create-home --shell /bin/bash tester
USER tester
WORKDIR /home/tester

ENV PATH=/home/tester/.cargo/bin:/home/tester/.bun/bin:$PATH \
    MIDWRITE_PG_BIN=/usr/lib/postgresql/15/bin

RUN curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal --component rustfmt,clippy --default-toolchain stable \
 && curl -fsSL https://bun.sh/install | bash -s "bun-v1.3.3"
