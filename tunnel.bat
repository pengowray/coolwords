@echo off
REM ===========================================================================
REM  coolwords - expose the local server on the public internet via a Cloudflare
REM  "quick tunnel" (no account / DNS needed). Prints a https://<random>.trycloudflare.com
REM  URL that proxies to the server below. Ctrl+C to stop (the URL dies with it).
REM
REM  Run serve.bat in another window first. The URL is PUBLIC and UNAUTHENTICATED —
REM  anyone with it can browse, import, and delete books. Keep it private; for a
REM  durable, access-controlled tunnel use a named tunnel + Cloudflare Access.
REM ===========================================================================
REM  --config points at an empty file on purpose: this box's default
REM  ~/.cloudflared/config.yml has a catch-all `http_status:404` ingress that a
REM  quick tunnel would otherwise inherit (→ 404 on every request). The empty
REM  config forces a clean quick tunnel that proxies straight to the origin.
set PORT=%1
if "%PORT%"=="" set PORT=7575
cloudflared tunnel --config "%~dp0.cf-empty.yml" --url http://127.0.0.1:%PORT%
