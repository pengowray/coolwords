# Coolwords — Home Assistant add-on

Runs the coolwords web app (Leptos/axum SSR + the Python importer with PyMuPDF +
Tesseract OCR) as a Home Assistant add-on, fronted by a Cloudflare tunnel +
Cloudflare Access (Google login) at `words.ffff.network`.

The big data (the 804 MB dictionary, the 277 MB embeddings, your tags, and
imported books) lives in `/share/coolwords/` — **not** in the image — so it
persists across every add-on update and updates stay small/fast.

---

## One-time setup

### 0. Prerequisites
- HAOS on x86/64 (this add-on builds `amd64`).
- `ghcr.io/pengowray/coolwords-amd64` exists (pushed by the GitHub Action). If your
  GitHub user/repo differs, change the owner in `coolwords/config.yaml` (`image:`)
  and re-run CI.

### 1. Seed `/share/coolwords/`
Install the **Samba share** (or **SSH/Terminal**) add-on, then copy from your dev box:

```
\\<homeassistant>\share\coolwords\coolwords.db        # required (804 MB)
\\<homeassistant>\share\coolwords\coolwords_emb.npy   # for import (clustering + L3 stemming)
\\<homeassistant>\share\coolwords\user.db             # optional; your existing tags
\\<homeassistant>\share\coolwords\books\              # optional; existing imported files
```

`user.db` and `books/` are created on first run if absent. You do **not** need the
4.5 GB fastText source — only the `.npy` it produces.

### 2. Add this repo as an add-on repository
Settings → Add-ons → Add-on Store → ⋮ (top-right) → **Repositories** →
add `https://github.com/pengowray/coolwords` → the **Coolwords** add-on appears.

### 3. Let the Supervisor pull the private image
The image is private (private repo). Give the Supervisor a GHCR read token once.
Create a GitHub PAT with **`read:packages`**, then (SSH/Terminal add-on):

```
ha registries add --hostname ghcr.io --username <your-gh-user> --password <PAT>
```

(Public repo? The package is public and this step is unnecessary.)

### 4. Install + start
Open the **Coolwords** add-on → **Install** → **Start**. Watch the log: it should
print `starting on 0.0.0.0:7575 (db=/share/coolwords/coolwords.db)`. Open the Web UI
from the add-on page to confirm it loads on the LAN before exposing it.

---

## Expose it at words.ffff.network (Cloudflared add-on + Access)

### A. Tunnel via the Cloudflared add-on
Install a **Cloudflared** HA add-on (e.g. the community `brenner-tobias` one). Point
a public hostname at this add-on's internal URL:

- Hostname: `words.ffff.network`
- Service: `http://<coolwords-internal-host>:7575`
  - With the Cloudflared add-on on the same HA host, `http://homeassistant.local:7575`
    or the add-on's hostname `http://addon_<slug>_coolwords:7575` works; the simplest
    is to enable "Show in sidebar"/ingress-less and use the host IP `http://<ha-ip>:7575`.

(Alternatively, add `words.ffff.network` as a public hostname on your existing
dashboard-managed token tunnel, service `http://<ha-ip>:7575` — same result.)

Add the `words` CNAME in Cloudflare DNS (proxied) if it isn't created automatically.

### B. Google login with Cloudflare Access
Cloudflare **Zero Trust** dashboard:
1. Settings → Authentication → add **Google** as a login method (one-time OAuth).
2. Access → Applications → **Add → Self-hosted**; domain `words.ffff.network`.
3. Policy: **Allow** → Include → **Emails:** `you@gmail.com` (or *Emails ending in*
   your domain).
4. Save. Every request now hits Google login first; the add-on only sees you after
   auth. (The app itself has no accounts — Access is the gate.)

---

## Updating the add-on (the fast path)

Code changes never recompile on the HAOS box and never re-ship the data:

1. `git push` your changes to `main` → the **build add-on image** Action compiles the
   `amd64` image (Rust deps cached) and pushes `:latest` + `:<version>` to GHCR.
2. Bump `version:` in `coolwords/config.yaml` to that new version and push.
3. In HA, the add-on shows **Update** → click it → the Supervisor pulls the new image
   (only changed layers, ~1–2 min). `/share` data — including your tags and books — is
   untouched.

Rolling back = set `version:` back to a previous tag and Update.

### What is and isn't shipped by an update
- **Shipped (in the image):** the server binary, the wasm/site assets, the `ingest/`
  Python, `schema/`. Small.
- **Never shipped (in `/share`):** `coolwords.db`, `coolwords_emb.npy`, `user.db`,
  `books/`. Regenerating the dictionary locally is a separate, infrequent ~800 MB copy
  to `/share` over Samba/SSH — independent of code updates. Books imported on the live
  site write straight into `/share/coolwords/books`.

---

## Notes / limitations
- **OCR engine:** Tesseract only in the image (RapidOCR omitted to stay small). Add
  `rapidocr-onnxruntime` to the Dockerfile's pip line if you want it.
- **Single user:** tags are the `me` rater in `user.db`. Access gates *who* can reach
  it, but everyone who passes shares one tag set.
- **Signals:** the server runs as PID 1; HA "Stop" sends SIGTERM and the container exits.
