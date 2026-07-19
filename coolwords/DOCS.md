# Coolwords add-on — exposing it at words.ffff.network

Install/deploy the add-on itself via [`../LOCAL.md`](../LOCAL.md) (local build, no
GitHub). This file only covers putting it behind a Cloudflare tunnel + Access (Google
login) so it's reachable at `words.ffff.network`.

The add-on serves plain HTTP on `:7575` on the LAN; Cloudflare Access is the auth gate
(the app itself has no accounts).

---

## A. Tunnel via the Cloudflared add-on
Install a **Cloudflared** HA add-on (e.g. the community `brenner-tobias` one). Point
a public hostname at this add-on's internal URL:

- Hostname: `words.ffff.network`
- Service: `http://<ha-ip>:7575` (simplest), or the add-on hostname
  `http://addon_local_coolwords:7575` if the Cloudflared add-on is on the same host.

(Alternatively, add `words.ffff.network` as a public hostname on your existing
dashboard-managed token tunnel, service `http://<ha-ip>:7575` — same result.)

Add the `words` CNAME in Cloudflare DNS (proxied) if it isn't created automatically.

## B. Google login with Cloudflare Access
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
