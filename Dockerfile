# syntax=docker/dockerfile:1
# ONE Dockerfile, two callers, both with the REPO ROOT as build context:
#   - Home Assistant local add-on build  (context = /addons/coolwords, i.e. this repo
#     copied onto the HAOS box; see config.yaml + LOCAL.md)
#   - GitHub Actions -> GHCR             (.github/workflows/build.yml, file: Dockerfile)
# The COPY paths below (ui/ schema/ ingest/ coolwords/run.sh) resolve from the root.

# ---- stage 1: compile the Leptos SSR server + hydrate wasm + site assets ----
FROM rust:1-bookworm AS build
RUN rustup target add wasm32-unknown-unknown \
 && cargo install cargo-leptos --locked
WORKDIR /src/ui

# Dependency layer. Compiling the ~400-crate graph (rusqlite builds SQLite from C;
# leptos is not small) is the bulk of the build, and it only ever changes when
# Cargo.toml/Cargo.lock do — but a single COPY of the whole crate keys it on every
# source edit too, so one typo costs a full rebuild. Build the deps against a STUB
# crate first: this layer then depends on the manifests alone, and editing app.rs
# recompiles one crate instead of all of them.
#
# NOT a BuildKit `RUN --mount=type=cache`: Home Assistant OS ships Docker with
# BuildKit disabled on purpose (home-assistant/operating-system#3935, wontfix), and
# under the classic builder `RUN --mount=` is a parse error, not an ignored flag.
# Plain layer caching is what both callers actually have.
COPY ui/Cargo.toml ui/Cargo.lock ./
RUN mkdir -p src \
 && echo 'fn main() {}' > src/main.rs \
 && : > src/lib.rs \
 && cargo build --release --no-default-features --features ssr --bin coolwords_ui \
 && cargo build --release --lib --target-dir=target/front \
      --target wasm32-unknown-unknown --no-default-features --features hydrate \
 && rm -rf src
# The wasm half needs --target-dir=target/front to match where cargo-leptos puts the
# front build; a future cargo-leptos that renames it would just miss this cache, not
# break the build.

# schema/ is needed here too: include_str! in app.rs (user.sql) and catalog.rs
# (catalog.sql). Copied as siblings of ui/ because those paths are ../../schema/.
COPY schema /src/schema
COPY ui/src ./src
COPY ui/style ./style
COPY ui/public ./public
# Cargo judges freshness by mtime, and COPY carries mtimes in from the build context
# — which can predate the stub artifacts above, leaving cargo satisfied that the stub
# is current and silently shipping an empty binary. Make the real sources newer.
RUN touch src/*.rs && cargo leptos build --release

# ---- stage 2: slim runtime = server binary + Python importer (PyMuPDF + Tesseract) ----
FROM debian:bookworm-slim
# numpy (scoring/clustering) + tesseract (OCR) from apt; PyMuPDF (PDF extract/raster)
# from pip wheels. jq parses the add-on options. rapidocr is intentionally omitted to
# keep the image small — tesseract is the default engine (add it later if wanted).
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      python3 python3-numpy python3-pip tesseract-ocr tesseract-ocr-eng ca-certificates jq bash \
 && pip3 install --no-cache-dir --break-system-packages pymupdf \
 && apt-get purge -y python3-pip \
 && apt-get autoremove -y \
 && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=build /src/ui/target/release/coolwords_ui /app/coolwords_ui
COPY --from=build /src/ui/target/site /app/site
COPY ingest /app/ingest
COPY schema /app/schema
COPY coolwords/run.sh /run.sh
RUN chmod a+x /run.sh /app/coolwords_ui
# Tell the server where its static assets live (cargo-leptos normally sets these).
ENV LEPTOS_OUTPUT_NAME=coolwords_ui \
    LEPTOS_SITE_ROOT=/app/site \
    LEPTOS_SITE_PKG_DIR=pkg \
    LEPTOS_ENV=PROD
CMD [ "/run.sh" ]
