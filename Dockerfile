FROM rust:latest as builder
WORKDIR /usr/src/priory
COPY . .
RUN cargo build --release

FROM rust:slim
COPY --from=builder /usr/src/priory/target/release/priory /usr/local/bin/priory
CMD ["priory"]

