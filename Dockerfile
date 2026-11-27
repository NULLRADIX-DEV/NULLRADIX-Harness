# syntax=docker/dockerfile:1

# Static web UI, built once for every platform
FROM --platform=$BUILDPLATFORM node:24-trixie-slim@sha256:4d00acb4bcc42c454c4eb801526f7ad7f772ea3bd4fd168c5cc2e384abcbb9d4 AS web
WORKDIR /src
COPY package.json package-lock.json ./
COPY web/package.json web/
COPY tests/e2e/package.json tests/e2e/
RUN npm ci --ignore-scripts
COPY web web
COPY assets assets
RUN npm run build

# Toolchain and dependency layers, so source edits do not rebuild the dependencies
FROM rust:1.100.0-trixie@sha256:e84068000361bf3d4a5ba2b7647f62893528b1eb9f847d7a23b493f5ce8ea254 AS chef
# The image ships this toolchain (keep in step with the tag), so ignore rust-toolchain.toml
ENV RUSTUP_TOOLCHAIN=1.100.0
RUN cargo install cargo-chef --locked --version 0.1.78
WORKDIR /src

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# The nullradix binary, built natively for the target platform
FROM chef AS build
COPY --from=planner /src/recipe.json recipe.json
RUN cargo chef cook --release --locked --recipe-path recipe.json -p nullradix-cli
COPY . .
RUN cargo build --release --locked -p nullradix-cli

FROM debian:trixie-slim@sha256:8aa5a7bf5a1955bddbe7d74fcdd0b91ab30e74362a8b73af672b0481b35b44a0 AS runtime
LABEL org.opencontainers.image.source="https://github.com/NULLRADIX-DEV/NULLRADIX-Harness"
# Runs and merges call git; the claude CLI is not included, mount or install it for dispatch
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates git \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --home-dir /var/lib/nullradix --create-home --shell /usr/sbin/nologin nullradix
COPY --from=build /src/target/release/nullradix /usr/local/bin/nullradix
COPY --from=web /src/web/dist /usr/local/share/nullradix/web
ENV NULLRADIX_HOME=/var/lib/nullradix \
    NULLRADIX_WEB_DIR=/usr/local/share/nullradix/web
VOLUME /var/lib/nullradix
USER nullradix
WORKDIR /var/lib/nullradix
# The daemon listens on loopback only, so reach it with host networking
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 CMD ["nullradix", "status"]
ENTRYPOINT ["nullradix"]
CMD ["serve"]
