# syntax=docker/dockerfile:1
#
# Image de VRC Sentinel Server :
# 1. construit l'interface d'administration (Svelte) ;
# 2. compile le serveur en binaire statique (musl), interface intégrée ;
# 3. livre une image minimale « distroless » : ni shell, ni gestionnaire de paquets,
#    utilisateur non root.

FROM node:22-alpine AS web
WORKDIR /web
COPY web/package.json web/package-lock.json ./
RUN npm ci --no-audit --no-fund
COPY web/ ./
RUN npm run build

FROM rust:1-alpine AS build
RUN apk add --no-cache musl-dev gcc
WORKDIR /src
# Dépendances compilées à part : gardées en cache tant que Cargo.toml ne change pas.
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src web/dist \
    && echo 'fn main() {}' > src/main.rs \
    && echo '<!doctype html>' > web/dist/index.html \
    && cargo build --release --locked \
    && rm -rf src
COPY src ./src
COPY --from=web /web/dist ./web/dist
RUN touch src/main.rs && cargo build --release --locked

FROM gcr.io/distroless/static-debian12:nonroot
COPY --from=build /src/target/release/sentinel-server /usr/local/bin/sentinel-server
USER nonroot:nonroot
EXPOSE 8080
ENV BIND=0.0.0.0:8080
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 \
    CMD ["/usr/local/bin/sentinel-server", "healthcheck"]
ENTRYPOINT ["/usr/local/bin/sentinel-server"]
CMD ["serve"]
