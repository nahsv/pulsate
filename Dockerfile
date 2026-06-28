# Build a static-ish Pulsate image: compile in a full toolchain, run on distroless.
FROM rust:1.86-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release --locked --bin pulsate --bin p8

# distroless/cc carries glibc + libgcc, which `ring` (TLS) needs.
FROM gcr.io/distroless/cc-debian12
COPY --from=build /src/target/release/pulsate /usr/bin/pulsate
COPY --from=build /src/target/release/p8 /usr/bin/p8
COPY packaging/pulsate.flow /etc/pulsate/pulsate.flow

# HTTP, metrics, admin.
EXPOSE 8080 9100 9180

ENTRYPOINT ["/usr/bin/pulsate"]
CMD ["up", "/etc/pulsate/pulsate.flow", "--listen", "0.0.0.0:8080", "--metrics", "0.0.0.0:9100", "--admin", "127.0.0.1:9180"]
