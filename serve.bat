@echo off
REM ===========================================================================
REM  coolwords - build then run the RELEASE web server (no live-reload).
REM  Just run this from the repo root; it builds with cargo-leptos first, then
REM  serves.  Ctrl+C to stop.  Pass --no-build to skip the build and run the
REM  existing binary:   serve.bat --no-build
REM
REM  Binds 127.0.0.1 by default (safe for a same-box Cloudflare tunnel). To expose
REM  on the LAN / for a remote tunnel, set COOLWORDS_BIND=0.0.0.0:7575 before running.
REM  Working dir stays in ui\ so the Python importer (repo ..\ingest) + ..\.env
REM  (COOLWORDS_BOOKS_DIR) resolve exactly as they do under `cargo leptos`.
REM ===========================================================================
cd /d "%~dp0ui"

if /i not "%~1"=="--no-build" (
    echo Building release ^(cargo leptos build --release^)...
    cargo leptos build --release || exit /b 1
)

set LEPTOS_OUTPUT_NAME=coolwords_ui
set LEPTOS_SITE_ROOT=target/site
set LEPTOS_SITE_PKG_DIR=pkg
set LEPTOS_ENV=PROD
if "%LEPTOS_SITE_ADDR%"=="" set LEPTOS_SITE_ADDR=127.0.0.1:7575

target\release\coolwords_ui.exe
