# --- build stage ---
FROM rust:1-bookworm AS builder
WORKDIR /app
COPY . .
RUN cargo build --release -p relay

# --- runtime stage ---
# The relay only listens (TLS is terminated by Fly), so a slim base with glibc is enough.
FROM debian:bookworm-slim
COPY --from=builder /app/target/release/relay /usr/local/bin/relay
# Fly injects PORT; default here matches the internal_port in fly.toml.
ENV PORT=8080
EXPOSE 8080
CMD ["relay"]
