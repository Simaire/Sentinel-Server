# syntax=docker/dockerfile:1
#
# Image de VRC Sentinel Server :
# 1. construit l'interface d'administration (Svelte) ;
# 2. compile le serveur en binaire statique (musl), interface intégrée ;
# 3. livre une image minimale « distroless » : ni shell, ni gestionnaire de paquets,
#    utilisateur non root.
#
# Les deux premières étapes tournent sur la machine qui construit ($BUILDPLATFORM), sans
# émulation : l'interface ne dépend pas du processeur, et le serveur est compilé en croisé
# (cargo-zigbuild, zig servant de compilateur C pour zstd) vers la cible demandée
# ($TARGETARCH). Un PC ARM64 produit ainsi l'image x64 en quelques minutes :
#   docker buildx build --platform linux/amd64,linux/arm64 -t … --push .

FROM --platform=$BUILDPLATFORM node:22-alpine AS web
WORKDIR /web
COPY web/package.json web/package-lock.json ./
RUN npm ci --no-audit --no-fund
COPY web/ ./
RUN npm run build

FROM --platform=$BUILDPLATFORM rust:1-alpine AS build
RUN apk add --no-cache musl-dev zig && cargo install cargo-zigbuild --locked
ARG TARGETARCH
RUN case "$TARGETARCH" in \
      amd64) echo x86_64-unknown-linux-musl ;; \
      arm64) echo aarch64-unknown-linux-musl ;; \
      *) echo "architecture non prise en charge : $TARGETARCH" >&2; exit 1 ;; \
    esac > /rust-target \
    && rustup target add "$(cat /rust-target)"
WORKDIR /src
# Dépendances compilées à part : gardées en cache tant que Cargo.toml ne change pas.
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src web/dist \
    && echo 'fn main() {}' > src/main.rs \
    && echo '<!doctype html>' > web/dist/index.html \
    && cargo zigbuild --release --locked --target "$(cat /rust-target)" \
    && rm -rf src
COPY src ./src
COPY --from=web /web/dist ./web/dist
RUN touch src/main.rs \
    && cargo zigbuild --release --locked --target "$(cat /rust-target)" \
    && cp "target/$(cat /rust-target)/release/sentinel-server" /sentinel-server

FROM gcr.io/distroless/static-debian12:nonroot
COPY --from=build /sentinel-server /usr/local/bin/sentinel-server
USER nonroot:nonroot
EXPOSE 8080
ENV BIND=0.0.0.0:8080
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 \
    CMD ["/usr/local/bin/sentinel-server", "healthcheck"]
ENTRYPOINT ["/usr/local/bin/sentinel-server"]
CMD ["serve"]
