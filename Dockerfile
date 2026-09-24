# syntax=docker/dockerfile:1

# ---------------------------------------------------------------------------
# Builder stage: compile the two lockpick binaries in a full Rust toolchain.
#
# The project uses edition 2024, so Rust 1.85 is the documented minimum. Keep
# the pin at 1.85 to match that requirement. If you need a newer toolchain,
# use a newer stable tag (e.g. "1.86-bookworm") — do NOT downgrade below 1.85.
# ---------------------------------------------------------------------------
FROM rust:1.98-bookworm AS builder

# Native build dependencies:
#   - build-essential: gcc/g++ (C++17 for RocksDB)
#   - cmake:          RocksDB build system
#   - clang / libclang-dev: required by the clang-sys crate (oxidowl -> oxigraph)
#   - pkg-config:     helps locate the above libraries
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        build-essential \
        cmake \
        clang \
        libclang-dev \
        pkg-config \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Copy the whole workspace (target/ is excluded via .dockerignore) and build
# both binaries in release mode.
COPY . .
RUN cargo build --release --bin lockpick --bin lockpick-lsp

# ---------------------------------------------------------------------------
# Runtime stage: a minimal image with only the compiled binaries.
# No Rust, Cargo, C++ toolchain, or build-time libraries are included.
# ---------------------------------------------------------------------------
FROM debian:bookworm-slim

# ca-certificates so the tools can reach HTTPS endpoints (e.g. remote ontologies).
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Run as an unprivileged user.
RUN useradd --create-home --uid 1000 --shell /bin/bash lockpick

# Copy the two binaries out of the builder stage.
COPY --from=builder /build/target/release/lockpick /usr/local/bin/lockpick
COPY --from=builder /build/target/release/lockpick-lsp /usr/local/bin/lockpick-lsp

USER lockpick
WORKDIR /work

# No ENTRYPOINT on purpose: the image can be run one-shot
#   docker run --rm -v "$PWD:/work" -w /work lockpick lockpick check ...
# or kept alive for `docker exec` workflows:
#   docker run -d --name lockpick -v "$PWD:/work" -w /work lockpick sleep infinity
CMD ["bash"]
