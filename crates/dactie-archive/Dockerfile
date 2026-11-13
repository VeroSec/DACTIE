# Use the official Rust image as the base image
# Pinned like rust-toolchain.toml (see the reason there). Upgrade both together.
FROM rust:1.100

# Set the working directory inside the container
WORKDIR /usr/src/dactie

COPY . .
# Build the Rust application
RUN cargo build --release -p dactie-archive

CMD ["/bin/sh"]
