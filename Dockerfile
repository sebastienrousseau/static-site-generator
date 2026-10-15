# SPDX-License-Identifier: Apache-2.0 OR MIT
#
# Two-stage build: compile with the full Rust toolchain, then copy the
# binary into a minimal Debian runtime image.
#
# Image: ghcr.io/sebastienrousseau/static-site-generator
# Built and pushed by .github/workflows/release.yml on every `v*` tag.

# ── Stage 1: build ─────────────────────────────────────────────────
FROM rust:1.89-slim@sha256:8cffb8fe4e8a95cf0d6a2060375e5a28aff4c752155aa9f1f9193530769bdf66 AS builder

WORKDIR /usr/src/ssg

# hadolint ignore=DL3008
RUN apt-get update \
 && apt-get install -y --no-install-recommends pkg-config libssl-dev perl make \
 && rm -rf /var/lib/apt/lists/*

# Copy everything and build. No dep-caching trick — the workspace's
# [patch.crates-io] section makes skeleton builds unreliable.
COPY . .
RUN cargo build --release --locked --bin ssg

# ── Stage 2: runtime ───────────────────────────────────────────────
FROM debian:bookworm-slim@sha256:60eac759739651111db372c07be67863818726f754804b8707c90979bda511df AS runtime

LABEL org.opencontainers.image.title="static-site-generator"
LABEL org.opencontainers.image.description="A Content-First Open Source Static Site Generator (SSG) crafted in Rust"
LABEL org.opencontainers.image.source="https://github.com/sebastienrousseau/static-site-generator"
LABEL org.opencontainers.image.licenses="MIT OR Apache-2.0"
LABEL org.opencontainers.image.authors="Sebastien Rousseau <contact@sebastienrousseau.com>"

# hadolint ignore=DL3008
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates libssl3 \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --create-home --shell /sbin/nologin --uid 10001 ssg

COPY --from=builder /usr/src/ssg/target/release/ssg /usr/local/bin/ssg

USER ssg
WORKDIR /workspace
EXPOSE 8000

ENTRYPOINT ["/usr/local/bin/ssg"]
CMD ["--help"]
