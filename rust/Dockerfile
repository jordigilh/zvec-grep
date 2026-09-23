ARG RUST_VERSION=1.98.0
FROM rust:${RUST_VERSION}-trixie

ENV CARGO_TERM_COLOR=always

RUN apt-get update \
    && apt-get install --no-install-recommends --yes \
        build-essential \
        ca-certificates \
        clang \
        cmake \
        git \
        libclang-dev \
        libssl-dev \
        pkg-config \
        python3 \
        perl \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /src
COPY . .
RUN cargo build --locked --release --package zg

ENTRYPOINT ["/src/target/release/zg"]
