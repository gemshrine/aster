# Build context: repository root (see docker-compose.yml).
FROM rust:1-slim-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN cargo build --release --locked -p signaling-server

FROM debian:bookworm-slim
RUN useradd --system --uid 10001 --no-create-home aster
COPY --from=build /src/target/release/signaling-server /usr/local/bin/signaling-server
USER aster
ENV PORT=8080
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/signaling-server"]
