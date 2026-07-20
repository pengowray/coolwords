# Coolwords add-on — access model & exposing it publicly

Install/deploy the add-on itself via [`../LOCAL.md`](../LOCAL.md) (local build, no
GitHub). This file covers *how you reach it*: privately through the Home Assistant
sidebar (default), and optionally publicly at `words.ffff.network` via a Cloudflare
tunnel + Access (Google login).

---

## How access works (the three "7575"s)

The add-on's [`config.yaml`](config.yaml) sets three ports that all happen to be `7575`
but live at different layers — this is the usual source of confusion:

| Setting | Layer | Meaning |
|---|---|---|
| `bind: 0.0.0.0:7575` | inside the container | address the server listens on. Must be `0.0.0.0` so ingress / other add-ons can reach it. |
| `ingress_port: 7575` | HA → container (internal) | the port HA's **ingress** proxy uses for the sidebar. Never touches the LAN. |
| `ports: 7575/tcp: null` | container → your LAN | publishes the port on the HA host IP. `null` = **not published** = invisible on the LAN. |

**Out of the box the only way in is the authenticated HA sidebar** (`ingress`). Nothing
is exposed. "Public or not" isn't a toggle inside the add-on — it's simply whether you
run a Cloudflare tunnel (below). Private stays the default.

### Sidebar (private, no setup)
Because `ingress: true`, once the add-on is **installed and started** it appears in the
HA sidebar as **Coolwords**, authenticated by Home Assistant. If it's missing: make sure
the running add-on is *this* local build (not the old GHCR image-based install — see the
recovery steps in [`../LOCAL.md`](../LOCAL.md)), that it's started, and that "Show in
sidebar" is on in the add-on's info page.

---

## Exposing it at words.ffff.network

The tunnel does **not** require publishing the LAN port. The Cloudflared add-on runs on
the same internal Docker network and reaches Coolwords by container hostname, so `ports`
stays `null` (zero LAN exposure) and Cloudflare Access is the only gate. The tunnel hits
the container directly (not through ingress), so the app serves at the root and asset
paths resolve normally — nothing to configure app-side.

### A. Tunnel via the Cloudflared add-on
1. Install a **Cloudflared** HA add-on (e.g. the community `brenner-tobias` one:
   Store → ⋮ → Repositories → `https://github.com/brenner-tobias/addon-cloudflared`).
2. Connect it to your Cloudflare account once (its login flow, or a tunnel token from
   the Zero Trust dashboard) per that add-on's docs.
3. Route the hostname to Coolwords via the add-on's **`additional_hosts`** option:
   ```yaml
   additional_hosts:
     - hostname: words.ffff.network
       service: http://addon_local_coolwords:7575
   ```
   `addon_local_coolwords` is this local add-on's internal hostname — confirm it on the
   Coolwords add-on's info page under **Hostname**.
4. Ensure a **proxied `words` CNAME** exists in Cloudflare DNS (the add-on usually
   creates it; otherwise add it manually pointing at your tunnel).

**Simpler alternative (also LAN-exposed):** set `ports: 7575/tcp: 7575` (or a host port
in the add-on's Network panel) and use `service: http://<ha-ip>:7575`. Easier to reason
about, but then the app is also reachable unauthenticated on your LAN.

### B. Google login with Cloudflare Access
Cloudflare **Zero Trust** dashboard:
1. Settings → Authentication → add **Google** as a login method (one-time OAuth).
2. Access → Applications → **Add → Self-hosted**; domain `words.ffff.network`.
3. Policy: **Allow** → Include → **Emails:** `you@gmail.com` (or *Emails ending in*
   your domain).
4. Save. Every request now hits Google login first; the add-on only sees you after auth.

---

## Notes / limitations
- **OCR engine:** Tesseract only in the image (RapidOCR omitted to stay small). Add
  `rapidocr-onnxruntime` to the `Dockerfile` pip line if you want it.
- **Single user:** tags are the `me` rater in `user.db`. Access gates *who* can reach
  it, but everyone who passes shares one tag set.
- **Signals:** the server runs as PID 1; HA "Stop" sends SIGTERM and the container exits.
