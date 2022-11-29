FROM rust:alpine as builder

RUN apk add --no-cache \
    musl-dev \
    openssl-dev

WORKDIR /usr/src/spotbot
COPY . .
RUN cargo install --path .

FROM alpine:latest

COPY --from=builder /usr/local/cargo/bin/spotbot /usr/local/bin/spotbot
CMD ["spotbot"]
