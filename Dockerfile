FROM debian:bullseye-slim

COPY target/x86_64-unknown-linux-musl/release/hoorayhug /usr/bin
RUN chmod +x /usr/bin/hoorayhug

ENTRYPOINT ["/usr/bin/hoorayhug"]
