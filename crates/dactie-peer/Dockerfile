# Use the official Rust image as the base image
FROM rust:latest

# Set the working directory inside the container
WORKDIR /usr/src/dactie

COPY . .
# Build the Rust application.
# FEATURES is empty by default; the compose peers pass "bench" to compile in
# the BENCH load generator, which is otherwise absent from the binary.
ARG FEATURES=""
RUN cargo build --release -p dactie-peer ${FEATURES:+--features $FEATURES}

CMD ["/bin/sh"]
