FROM node:22-bookworm-slim AS frontend-build
WORKDIR /app/frontend

COPY frontend/package.json frontend/package-lock.json ./
RUN npm ci

COPY frontend/ ./
RUN npm run build


FROM rust:1.93-bookworm AS backend-build
WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY src ./src

RUN cargo build --release


FROM debian:bookworm-slim AS runtime
WORKDIR /app

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=backend-build /app/target/release/train-stations-map /usr/local/bin/train-stations-map
COPY --from=frontend-build /app/frontend/dist /opt/train-stations-map/frontend/dist

ENV FRONTEND_DIST=/opt/train-stations-map/frontend/dist
EXPOSE 3000

CMD ["train-stations-map"]