FROM rust:1.88-bookworm AS builder
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY migrations ./migrations
RUN cargo build --release -p mock-psp -p invoice-service

FROM debian:bookworm-slim AS mock-psp
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/mock-psp /usr/local/bin/mock-psp
ENV PORT=8081
EXPOSE 8081
CMD ["mock-psp"]

FROM debian:bookworm-slim AS invoice-service
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/invoice-service /usr/local/bin/invoice-service
ENV LISTEN_ADDR=0.0.0.0:8080
ENV DATABASE_URL=postgres://postgres:postgres@postgres:5432/invoices
ENV PSP_URL=http://mock-psp:8081/charge
EXPOSE 8080
CMD ["invoice-service"]
