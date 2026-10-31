# All tool installations are isolated from the host. Runtime builds link on Ubuntu 22.04.
FROM golang:1.24-bookworm@sha256:1a6d4452c65dea36aac2e2d606b01b4a029ec90cc1ae53890540ce6173ea77ac AS workflow-lint
RUN CGO_ENABLED=0 go install github.com/rhysd/actionlint/cmd/actionlint@v1.7.7

FROM rust:1.99.1-slim-bookworm@sha256:54e39a2f4480fd827ebfa47736212703a1e46be0fddf7e1590aa0e94b8d0cae1 AS rust-toolchain
RUN rustup component add rustfmt clippy

FROM ghcr.io/astral-sh/uv:0.9.27@sha256:143b40f4ab56a780f43377604702107b5a35f83a4453daf1e4be691358718a6a AS uv

FROM ubuntu:22.04@sha256:5ec03bb3441e8b0bf3b4f9cd4629a1ae763010dc3035bb8da3ae6cf026486401
RUN apt-get update && apt-get install --yes --no-install-recommends \
    build-essential ca-certificates curl git pkg-config libssl-dev \
    libclang-14-dev=1:14.0.0-1ubuntu1.1 clang-14=1:14.0.0-1ubuntu1.1 \
    && rm -rf /var/lib/apt/lists/*
COPY --from=rust-toolchain /usr/local/cargo /opt/cargo
COPY --from=rust-toolchain /usr/local/rustup /opt/rustup
COPY --from=workflow-lint /go/bin/actionlint /usr/local/bin/actionlint
COPY --from=uv /uv /usr/local/bin/uv
ENV CARGO_HOME=/opt/cargo RUSTUP_HOME=/opt/rustup PATH=/opt/checks/bin:/opt/cargo/bin:$PATH
# bindgen of mupdf-sys (DEP-29) finds libclang here.
ENV LIBCLANG_PATH=/usr/lib/llvm-14/lib
COPY tools/requirements-*.txt /tmp/requirements/
RUN uv python install 3.12.12 \
    && uv venv --python 3.12.12 /opt/checks \
    && uv pip install --python /opt/checks/bin/python -r /tmp/requirements/requirements-ci.txt \
    && cargo install cargo-audit --version 0.22.2 --locked \
    && cargo install cargo-about --version 0.9.2 --features cli --locked
WORKDIR /workspace
CMD ["bash"]
