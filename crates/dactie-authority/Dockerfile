# Use the official Rust image as the base image
# Pinned like rust-toolchain.toml: Rust 1.100 no longer compiles sqlx 0.7 (never-type
# fallback, rust-lang/rust#148922). Upgrade both together, with sqlx 0.8.
FROM rust:1.99

# Set the working directory inside the container
WORKDIR /usr/src/dactie

COPY . .
# Build the Rust application
RUN cargo build --release -p dactie-authority

CMD ["/bin/sh"]
