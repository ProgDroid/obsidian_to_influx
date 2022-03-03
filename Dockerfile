FROM lukemathwalker/cargo-chef:latest-rust-1.53.0 as chef
WORKDIR /app

FROM chef as planner
COPY . .
RUN cargo chef prepare  --recipe-path recipe.json

FROM chef as builder
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

COPY . .
RUN cargo build --release --bin obsidian_to_influx

FROM debian:bullseye-slim AS runtime
WORKDIR /app
COPY --from=builder /app/target/release/obsidian_to_influx obsidian_to_influx
ENTRYPOINT ["./obsidian_to_influx"]
