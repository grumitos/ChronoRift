# ADR-007: Serve DataDragon and CommunityDragon Through a Disk Cache

## Status

Accepted

## Date

2026-09-29

## Context

The match view loads champion, item, spell and rune data and images from DataDragon and CommunityDragon. The webview requested them directly on every run. Only the WebView2 HTTP cache kept copies, and Windows or a profile reset can clear it. The user wants each file downloaded once and reused from disk.

A match needs about 150 images at once, and the same item icon appears on several players. The locale fallback (`es_mx`, then `default`) also produced a 404 on every run.

## Decision

- Register an asynchronous `lolasset` URI scheme in Rust. On Windows the webview sees it as `http://lolasset.localhost/<source>/<path>`, where `source` is `ddragon` or `cdragon`. No other origin is reachable.
- Store files under `<app cache dir>\assets\<source>\<path>`. Every path segment is percent-decoded and validated as a Windows file name before touching the disk. Writes go to a temporary file and are renamed into place.
- A path with a version (`cdn/16.18.1/…` or `16.18/…`) never changes: it is downloaded once and then always served from disk. Other paths (`api/versions.json`) are refreshed after 12 hours, and served stale if the network fails. A 404 on a versioned path is remembered for 24 hours.
- One download per file at a time (a lock per path), and at most six downloads at once.
- The UI maps DataDragon and CommunityDragon URLs to the scheme in two places: `assetImage()` for images and the `fetch` calls in `game-assets.ts`. The CSP no longer allows the CDNs, so a missed URL fails visibly instead of silently skipping the cache.

## Alternatives Considered

### Rely on the WebView2 HTTP cache

Rejected. It is not under the app's control: headers, eviction and profile resets decide what stays.

### Cache Storage in the webview

Rejected. It lives in the same WebView2 profile, needs images to be fetched as blobs and turned into object URLs, and does not survive a profile reset.

### Tauri's `asset:` protocol plus a download command

Rejected. Every image would need an `invoke` round trip before its `<img>` could be created, and the asset protocol would need a filesystem scope.

## Consequences

- After the first view of a patch, its matches render from disk, also without a network connection.
- The cache grows with each patch seen (a few MB per patch). There is no "clear cache" control yet; the folder can be deleted safely.
- `tokio` (already in the dependency tree through Tauri) becomes a direct dependency with the `sync` feature, for the per-path lock and the download limit.

## Update 2026-09-29

- DataDragon is an S3 bucket and answers a missing file with 403, not 404. On a versioned DataDragon path a 403 now counts as a 404, so it is remembered for 24 hours like any other miss, also across sessions. A 403 from CommunityDragon, or on a path without a version, stays an upstream error.
- A remembered miss only counts for the exact file name. NTFS ignores case but the CDNs do not, so a miss for one spelling never hides a file with another.
- Redirects are only followed within the origin (scheme, host, and port) of the request. None of the paths checked on 2026-09-29 redirected: DataDragon `api/versions.json`, `cdn/16.18.1/img/…` and `cdn/16.18.1/data/es_MX/champion.json`, and CommunityDragon `16.18/plugins/rcp-be-lol-game-data/global/…/v1/*.json`.
- The webview treats `http://lolasset.localhost` as a local origin, and the scheme serves third-party SVGs. Every response now carries `Content-Security-Policy: default-src 'none'; img-src 'self' data:; style-src 'unsafe-inline'; sandbox`, so an SVG opened as a document runs no script and gets an opaque origin. The header changes nothing for `fetch` and `<img>`.
- CORS is no longer open to every origin (`*`). `Access-Control-Allow-Origin` repeats the request's `Origin` only when it is one of the app's origins: `http://tauri.localhost` (the packaged app on Windows), `https://tauri.localhost`, `tauri://localhost`, and Vite's `http://localhost:1420`. Errors carry it too, so the UI still sees a 404 and tries the next locale. Any other origin, `null` included, gets no CORS header, and every response carries `Vary: Origin`.
- A request without `Origin` gets the UI's own origin: `http://tauri.localhost` in the packaged app and `http://localhost:1420` in development, the same test Tauri uses to pick `devUrl` or the bundled files. An `<img>` sends no `Origin`. WebView2 passes `Origin` to custom-scheme handlers on POST requests, since Tauri's IPC handler requires it, but the JSON fetches are cross-origin GETs. Chromium adds `Origin` to those in its network service, not in the page, and whether a WebView2 handler sees it was not confirmed. The browser compares this fixed value with the requester's origin, so no other origin gains access.
- If the HTTP client with the cache's policy (timeouts, redirects within the origin) cannot be built, the cache no longer falls back to a default client, which has neither. It stays offline: files on disk are served, and any other request is an upstream error that is not remembered as missing.
