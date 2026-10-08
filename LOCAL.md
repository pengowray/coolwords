# Coolwords — local Home Assistant add-on (no GitHub, HAOS builds it)

This runs the add-on **without GHCR or GitHub**: you copy the repo onto the HAOS box
and the Supervisor compiles the image locally from [`Dockerfile`](Dockerfile). The
manifest that makes this a *local build* is [`config.yaml`](config.yaml) at the repo
root — it has **no `image:` key**, which is what tells the Supervisor to build instead
of pull.

Data (`coolwords.db`, `coolwords_emb.npy`, `user.db`, `books/`) lives in
`/share/coolwords/`, not in the image, so it survives every rebuild.

> **Heads-up on the build:** this compiles Rust + `cargo-leptos` + wasm on the HAOS
> box. Budget **20–60+ min** on first build and make sure the machine has a few GB of
> free disk and enough RAM (a low-RAM box can OOM mid-compile). Rebuilds after code
> changes are much faster: the dependency graph is compiled in its own layer keyed
> only on `ui/Cargo.toml` + `ui/Cargo.lock`, so editing app code recompiles one crate
> instead of ~400. Only a dependency change (or HA discarding build layers, which it
> may do between reboots) brings back the full cost.
>
> Note the Dockerfile deliberately uses **plain layer caching, not** BuildKit's
> `RUN --mount=type=cache` — HAOS ships Docker with BuildKit disabled on purpose
> ([operating-system#3935](https://github.com/home-assistant/operating-system/issues/3935),
> closed wontfix), and under the classic builder `RUN --mount=` fails to parse.

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
[coolwords] 0.1.0 (built 2026-08-05T01:23:45Z) starting at 2026-08-05T01:24:02Z
listening on http://0.0.0.0:7575
```
Open the **Web UI** link on the add-on page to confirm it loads on the LAN.

### Already installed the old GHCR (image-based) version?
If the Supervisor is failing with `ghcr.io/...: [401] unauthorized`, an image-based
install is still registered and editing files won't switch it to local-build. Reset it:

1. **Uninstall** the Coolwords add-on in HA (removes the image-based registration; your
   `/share` data is untouched).
2. On the box, make sure `/addons/coolwords/` holds the **repo root** — i.e.
   `/addons/coolwords/config.yaml` is the one with **no `image:` line** (`grep -i image
   /addons/coolwords/config.yaml` should print nothing). If you copied only the inner
   folder, delete `/addons/coolwords/` and re-copy the whole repo.
3. Reload: Add-on Store → ⋮ → **Check for updates** (or restart the Supervisor:
   `ha supervisor reload`, or Settings → System → ⋮ → *Restart Supervisor*).
4. Open **Coolwords (local build)** under **Local add-ons** → **Install** (this builds).

---

## Updating (code changes, still no GitHub)
1. Re-copy the changed source into `/addons/coolwords/` (or `git pull` on the box).
2. On the add-on page: ⋮ → **Rebuild**. The Supervisor recompiles from the local
   `Dockerfile`; `/share` data is untouched.

Bumping `version:` in the root `config.yaml` isn't required for local rebuilds, but if
you do bump it HA will surface a normal **Update** button too.

### Checking which build is running
Home Assistant's add-on log has no timestamps or version numbers, so the server
reports its version and build time in two places:

- **The log**, on every start: the `[coolwords] <version> (built ...)` line right
  after the `[coolwords] starting on ...` line, for example
  `[coolwords] 0.1.0 (built 2026-08-05T01:23:45Z) starting at 2026-08-05T01:24:02Z`
- **`/version`** on the running server. Open it from the sidebar panel URL (append
  `/version`) or through the tunnel at `https://words.ffff.network/version`. It
  returns plain text with the version, build time, start time and current time, so
  you can also see how long the server has been running.

All times are UTC. "built" is the time of the last rebuild that changed a file in
the image, such as after an edit under `ui/`, `ingest/` or `schema/`, or to
`coolwords/run.sh`. A rebuild with no changed files keeps the earlier time, so if
"built" is older than your latest rebuild, your changes probably did not reach
`/addons/coolwords/` on the box. Check the files there and rebuild again.

The version number comes from `ui/Cargo.toml`. Bumping `version:` in `config.yaml`
changes neither the version number nor "built".

---

## Access (private by default) + exposing it
Once started, the add-on is reachable **privately through the HA sidebar** (ingress) —
no ports exposed. To also put it behind a Cloudflare tunnel + Access (Google login) at
`words.ffff.network`, and for how the sidebar / ports / public toggle fit together, see
[`coolwords/DOCS.md`](coolwords/DOCS.md).

---

## Notes
- **Reference datasets** (CMUdict, ngrams, WordNet and so on) are needed only to
  build `coolwords.db`, which you do on your own machine, not on the HAOS box. The
  ingest scripts read them from the folder set by `COOLWORDS_DATASETS` (an
  environment variable, or a line in the repo-root `.env`); see [README.md](README.md).
- **OCR engine:** Tesseract only (RapidOCR omitted to keep the build small). Add
  `rapidocr-onnxruntime` to the `Dockerfile` pip line if you want it.
- **Single user:** tags are the `me` rater in `user.db`; put Cloudflare Access in front
  to gate *who* can reach it.
- **Signals:** the server runs as PID 1; HA "Stop" sends SIGTERM and the container exits.
