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
WORKDIR /src
# schema/ is needed here too: app.rs include_str!("../../schema/user.sql").
COPY ui ./ui
COPY schema ./schema
WORKDIR /src/ui
RUN cargo leptos build --release

# ---- stage 2: slim runtime = server binary + Python importer (PyMuPDF + Tesseract) ----
FROM debian:bookworm-slim
# numpy (scoring/clustering) + tesseract (OCR) from apt; PyMuPDF (PDF extract/raster)
# from pip wheels. jq parses the add-on options. rapidocr is intentionally omitted to
# keep the image small — tesseract is the default engine (add it later if wanted).
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      python3 python3-numpy python3-pip tesseract-ocr ca-certificates jq bash \
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
