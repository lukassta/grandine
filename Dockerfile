FROM rust:1.95.0-bookworm AS builder
RUN apt-get update && apt-get --yes upgrade && apt-get install --yes cmake libclang-dev
COPY . .
# RUN scripts/build/release.sh
RUN make minimal

FROM ubuntu:latest
COPY --from=builder /target/compact/grandine /usr/local/bin/grandine

ENTRYPOINT ["grandine"]
