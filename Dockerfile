# syntax=docker/dockerfile:1
FROM node:24-alpine AS frontend
WORKDIR /build/frontend
COPY frontend/package.json frontend/package-lock.json ./
RUN npm ci
COPY frontend/ ./
RUN npm run build

FROM rust:1.96-alpine AS backend
RUN apk add --no-cache musl-dev build-base binutils ca-certificates
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY backend/ ./backend/
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build/target \
    cargo build --release --locked -p taskboard && \
    mkdir -p /out /runtime/data /runtime/tmp && \
    cp target/release/taskboard /out/taskboard && \
    if readelf -l /out/taskboard | grep -q INTERP; then echo 'Binary must be statically linked'; exit 1; fi
RUN chown -R 10001:10001 /runtime/data /runtime/tmp && chmod 1777 /runtime/tmp

FROM scratch AS runtime
COPY --from=backend /out/taskboard /taskboard
COPY --from=backend /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
COPY --from=backend --chown=10001:10001 /runtime/data /data
COPY --from=backend --chown=10001:10001 /runtime/tmp /tmp
COPY --from=frontend /build/frontend/dist /www
USER 10001:10001
ENV TASKBOARD_DATABASE=/data/taskboard.db \
    TASKBOARD_FRONTEND=/www \
    TASKBOARD_BIND=0.0.0.0:8080
EXPOSE 8080
VOLUME ["/data"]
HEALTHCHECK --interval=30s --timeout=5s --start-period=15s --retries=3 CMD ["/taskboard", "healthcheck"]
ENTRYPOINT ["/taskboard"]

