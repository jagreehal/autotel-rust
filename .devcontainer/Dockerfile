FROM mcr.microsoft.com/devcontainers/rust:1-bookworm

RUN rustup toolchain install 1.97.1 --profile minimal --component clippy,rustfmt \
    && rustup default 1.97.1 \
    && cargo install cargo-audit --locked \
    && cargo install cargo-outdated --locked
