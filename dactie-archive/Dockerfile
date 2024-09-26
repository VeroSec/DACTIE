# Use the official Rust image as the base image
FROM rust:latest

# Set the working directory inside the container
WORKDIR /usr/src/dactie

COPY . .
# Build the Rust application
RUN cargo build --release -p dactie-archive

CMD ["/bin/sh"]
