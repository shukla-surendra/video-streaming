# syntax=docker/dockerfile:1
# Backend: Rust API + transcode worker (needs ffmpeg at runtime).

FROM rust:1.97-slim-trixie AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
# Cache mounts keep crates and compiled deps between builds; only our code recompiles.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked --bin vstream \
 && cp target/release/vstream /usr/local/bin/vstream

FROM debian:trixie-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ffmpeg \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --system --uid 10001 --no-create-home vstream \
 && mkdir /data && chown vstream /data
WORKDIR /app
COPY --from=build /usr/local/bin/vstream /usr/local/bin/vstream
# The backend still serves the pages itself, handy for hitting :3000 directly.
COPY static ./static
ENV VSTREAM_DATA=/data PORT=3000
USER vstream
EXPOSE 3000
VOLUME /data
HEALTHCHECK --interval=10s --timeout=3s --start-period=5s \
  CMD bash -c 'exec 3<>/dev/tcp/127.0.0.1/3000' || exit 1
CMD ["vstream"]
