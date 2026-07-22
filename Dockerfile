FROM rust:1.96-bookworm AS builder

RUN apt-get update \
    && apt-get install --yes --no-install-recommends build-essential cmake clang pkg-config \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY src ./src
RUN cargo build --locked --release

FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=builder /build/target/release/kse-static-rewrite-proxy /usr/local/bin/kse-static-rewrite-proxy

ENV KSE_REWRITE_CONFIG=/etc/kse-console/config.yaml
ENV RUST_LOG=info
EXPOSE 8080 9090
USER nonroot:nonroot
ENTRYPOINT ["/usr/local/bin/kse-static-rewrite-proxy"]
