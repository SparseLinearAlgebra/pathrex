FROM rust:1-bookworm AS builder

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        build-essential \
        ca-certificates \
        clang \
        cmake \
        git \
        libclang-dev \
        make \
        pkg-config \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /src

COPY . .

RUN git clone --depth 1 https://github.com/DrTimothyAldenDavis/GraphBLAS.git /tmp/GraphBLAS \
    && make -C /tmp/GraphBLAS compact \
    && make -C /tmp/GraphBLAS install \
    && mkdir -p deps/LAGraph/build \
    && make -C deps/LAGraph \
    && cargo build --release --bin pathrex --features "bench,regenerate-bindings"

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates \
        gcc \
        libc6-dev \
        libgomp1 \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /work

COPY --from=builder /src/target/release/pathrex /usr/local/bin/pathrex
COPY --from=builder /usr/local/lib/libgraphblas.so* /usr/local/lib/
COPY --from=builder /src/deps/LAGraph/build/src/liblagraph.so* /usr/local/lib/
COPY --from=builder /src/deps/LAGraph/build/experimental/liblagraphx.so* /usr/local/lib/
COPY --from=builder /src/docker/docker-entrypoint.sh /usr/local/bin/docker-entrypoint.sh

RUN chmod +x /usr/local/bin/docker-entrypoint.sh

ENV LD_LIBRARY_PATH=/usr/local/lib

ENTRYPOINT ["/usr/local/bin/docker-entrypoint.sh"]
CMD ["--help"]
