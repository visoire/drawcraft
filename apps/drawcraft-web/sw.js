/* DrawCraft offline shell.
 *
 * - Navigation and the WebAssembly bundle: network first, cached copy when offline, so the
 *   JS glue and the .wasm file are always from the same build while online.
 * - Icons, manifest and the storage adapter: cache first.
 * - Never intercepts cross-origin requests (sync servers keep their own credentials).
 * Bump CACHE_VERSION on every release so old caches are removed.
 */
const CACHE_VERSION = 'drawcraft-shell-v1';
const PRECACHE = [
  './',
  'index.html',
  'manifest.webmanifest',
  'storage_adapter.js',
  'icons/icon-192.png',
  'icons/icon-512.png',
  'icons/icon-maskable-512.png',
  'icons/apple-touch-icon-180.png',
  'icons/favicon-32.png',
];

self.addEventListener('install', (event) => {
  event.waitUntil(
    caches
      .open(CACHE_VERSION)
      .then((cache) => cache.addAll(PRECACHE))
      .then(() => self.skipWaiting())
  );
});

self.addEventListener('activate', (event) => {
  event.waitUntil(
    caches
      .keys()
      .then((keys) => Promise.all(keys.filter((k) => k !== CACHE_VERSION).map((k) => caches.delete(k))))
      .then(() => self.clients.claim())
  );
});

function isStaticAsset(url) {
  return /\/icons\/|\/manifest\.webmanifest$|\/storage_adapter\.js$/.test(url.pathname);
}

self.addEventListener('fetch', (event) => {
  const request = event.request;
  if (request.method !== 'GET') return;
  const url = new URL(request.url);
  if (url.origin !== self.location.origin) return;

  if (isStaticAsset(url)) {
    event.respondWith(
      caches.match(request).then((hit) => hit || fetch(request).then((res) => store(request, res)))
    );
    return;
  }

  // Everything else (index.html, drawcraft_app.js, drawcraft_app_bg.wasm): network first.
  event.respondWith(
    fetch(request)
      .then((res) => store(request, res))
      .catch(() =>
        caches.match(request).then((hit) => hit || (request.mode === 'navigate' ? caches.match('index.html') : Response.error()))
      )
  );
});

function store(request, response) {
  if (!response || !response.ok || response.type === 'opaque') return response;
  const copy = response.clone();
  caches.open(CACHE_VERSION).then((cache) => cache.put(request, copy)).catch(() => {});
  return response;
}
