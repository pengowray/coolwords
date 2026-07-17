# Coolwords — local Home Assistant add-on (no GitHub, HAOS builds it)

This runs the add-on **without GHCR or GitHub**: you copy the repo onto the HAOS box
and the Supervisor compiles the image locally from [`Dockerfile`](Dockerfile). The
manifest that makes this a *local build* is [`config.yaml`](config.yaml) at the repo
root — it has **no `image:` key**, which is what tells the Supervisor to build instead
of pull. (`coolwords/config.yaml` is the separate GHCR-pull variant; ignore it here.)

Data (`coolwords.db`, `coolwords_emb.npy`, `user.db`, `books/`) lives in
`/share/coolwords/`, not in the image, so it survives every rebuild.

> **Heads-up on the build:** this compiles Rust + `cargo-leptos` + wasm on the HAOS
> box. Budget **20–60+ min** on first build and make sure the machine has a few GB of
> free disk and enough RAM (a low-RAM box can OOM mid-compile). Rebuilds after code
> changes are faster (cargo cache), but HA may discard build layers between reboots.

---

## One-time setup

### 1. Get a shell/file access to the box
Install the **SSH & Web Terminal** add-on (turn *Protection mode* OFF so you get the
host Docker), and/or the **Samba share** add-on for drag-and-drop file copying.

### 2. Put the repo in `/addons/coolwords/`
The add-on folder must **be** the repo (the Dockerfile COPYs `ui/ schema/ ingest/
coolwords/run.sh` from the folder root). Two ways:

**Samba:** browse to `\\<homeassistant>\addons\`, create `coolwords\`, and copy the
repo contents in so you end up with:

```
\\<homeassistant>\addons\coolwords\config.yaml     <- root manifest (local build)
\\<homeassistant>\addons\coolwords\Dockerfile
\\<homeassistant>\addons\coolwords\ui\
\\<homeassistant>\addons\coolwords\ingest\
\\<homeassistant>\addons\coolwords\schema\
\\<homeassistant>\addons\coolwords\coolwords\run.sh
```

**SSH (git clone on the box):**
```bash
cd /addons && git clone <your-local-or-lan-git-url> coolwords
```

You do **not** need to copy `data/`, `target/`, `ui/target/`, `*.db`, `*.npy`, or
`books/` — `.dockerignore` keeps them out of the build anyway, and the data belongs in
`/share` (next step). Copying them just wastes space on the box.

### 3. Seed `/share/coolwords/`
Copy the data next to where the add-on will look for it:

```
\\<homeassistant>\share\coolwords\coolwords.db        # required (~804 MB)
\\<homeassistant>\share\coolwords\coolwords_emb.npy   # for import (clustering + L3 stemming)
\\<homeassistant>\share\coolwords\user.db             # optional; your existing tags
\\<homeassistant>\share\coolwords\books\              # optional; existing imported files
```

`user.db` and `books/` are created on first run if absent. You do **not** need the
multi-GB fastText source — only the `.npy` it produces.

### 4. Load + install the add-on
Settings → Add-ons → Add-on Store → ⋮ (top-right) → **Check for updates** (this
re-scans `/addons`; if it doesn't appear, restart the **Supervisor** from Settings →
System → ⋮). Under **Local add-ons** you'll see **Coolwords (local build)** → open it →
**Install**. The first install *is* the build — watch the log until it finishes, then
**Start**.

A healthy start logs:
```
[coolwords] starting on 0.0.0.0:7575 (db=/share/coolwords/coolwords.db)
```
Open the **Web UI** link on the add-on page to confirm it loads on the LAN.

---

## Updating (code changes, still no GitHub)
1. Re-copy the changed source into `/addons/coolwords/` (or `git pull` on the box).
2. On the add-on page: ⋮ → **Rebuild**. The Supervisor recompiles from the local
   `Dockerfile`; `/share` data is untouched.

Bumping `version:` in the root `config.yaml` isn't required for local rebuilds, but if
you do bump it HA will surface a normal **Update** button too.

---

## Exposing it (optional)
The Cloudflare tunnel + Access (Google login) at `words.ffff.network` is identical to
the GHCR flow — see the "Expose it" section in [`coolwords/DOCS.md`](coolwords/DOCS.md).
Point the tunnel's public hostname at `http://<ha-ip>:7575`.

---

## Notes
- **OCR engine:** Tesseract only (RapidOCR omitted to keep the build small). Add
  `rapidocr-onnxruntime` to the `Dockerfile` pip line if you want it.
- **Single user:** tags are the `me` rater in `user.db`; put Cloudflare Access in front
  to gate *who* can reach it.
- **Signals:** the server runs as PID 1; HA "Stop" sends SIGTERM and the container exits.
