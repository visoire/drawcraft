/* Persistence adapter for the WebAssembly app: OPFS first, IndexedDB as the fallback.
 *
 * Plain browser glue only. All decisions (what to save, when, what "saved" means, retries,
 * memory budgets) live in Rust (`drawcraft-storage` + `web_storage.rs`). Every function returns
 * a Promise and rejects with the original DOMException so Rust can tell QuotaExceededError and
 * permission errors apart.
 *
 * Why two backends: OPFS needs a secure context and `createWritable()`, which older Safari
 * versions lack; IndexedDB works everywhere (including plain-HTTP LAN testing). localStorage is
 * deliberately never used for notebooks (5 MB cap).
 */
(function () {
  'use strict';

  var DB_NAME = 'drawcraft';
  var STORE = 'blobs';
  var DIR_NAME = 'drawcraft';
  var backend = null; // 'opfs' | 'indexeddb' | 'none'
  var dirPromise = null;
  var dbPromise = null;

  function sanitizeKey(key) {
    // Keys are validated in Rust; flatten the separators for a single OPFS directory.
    return String(key).replace(/[^A-Za-z0-9._-]/g, '_');
  }

  function opfsDirectory() {
    if (!dirPromise) {
      dirPromise = navigator.storage.getDirectory().then(function (root) {
        return root.getDirectoryHandle(DIR_NAME, { create: true });
      });
    }
    return dirPromise;
  }

  /* An OPFS write is only used when createWritable() exists and a probe write succeeds. */
  function probeOpfs() {
    if (!window.isSecureContext || !navigator.storage || !navigator.storage.getDirectory) {
      return Promise.resolve(false);
    }
    return opfsDirectory()
      .then(function (dir) {
        return dir.getFileHandle('.probe', { create: true });
      })
      .then(function (handle) {
        if (typeof handle.createWritable !== 'function') return false;
        return handle.createWritable().then(function (w) {
          return w.write(new Uint8Array([1])).then(function () {
            return w.close();
          });
        }).then(function () { return true; });
      })
      .catch(function () { return false; });
  }

  function openDb() {
    if (!dbPromise) {
      dbPromise = new Promise(function (resolve, reject) {
        if (!window.indexedDB) {
          reject(new Error('IndexedDB is not available'));
          return;
        }
        var req = indexedDB.open(DB_NAME, 1);
        req.onupgradeneeded = function () {
          req.result.createObjectStore(STORE);
        };
        req.onsuccess = function () { resolve(req.result); };
        req.onerror = function () { reject(req.error); };
      });
    }
    return dbPromise;
  }

  function idbRun(mode, fn) {
    return openDb().then(function (db) {
      return new Promise(function (resolve, reject) {
        var tx = db.transaction(STORE, mode);
        var result;
        try { result = fn(tx.objectStore(STORE)); } catch (e) { reject(e); return; }
        // Resolve only when the transaction has committed: that is "persisted".
        tx.oncomplete = function () { resolve(result && 'result' in result ? result.result : undefined); };
        tx.onerror = function () { reject(tx.error); };
        tx.onabort = function () { reject(tx.error || new DOMException('Transaction aborted', 'AbortError')); };
      });
    });
  }

  window.drawcraftStorage = {
    /* Chooses the backend once and asks the browser to keep the data (best effort). */
    open: function () {
      if (backend) return Promise.resolve(backend);
      return probeOpfs().then(function (opfsOk) {
        if (opfsOk) return 'opfs';
        return openDb().then(function () { return 'indexeddb'; }, function () { return 'none'; });
      }).then(function (chosen) {
        backend = chosen;
        if (navigator.storage && navigator.storage.persist) {
          navigator.storage.persist().catch(function () {});
        }
        return backend;
      });
    },

    /* Resolves a Uint8Array, or null when the key does not exist. */
    get: function (key) {
      if (backend === 'opfs') {
        return opfsDirectory()
          .then(function (dir) { return dir.getFileHandle(sanitizeKey(key)); })
          .then(function (handle) { return handle.getFile(); })
          .then(function (file) { return file.arrayBuffer(); })
          .then(function (buf) { return new Uint8Array(buf); })
          .catch(function (err) {
            if (err && err.name === 'NotFoundError') return null;
            throw err;
          });
      }
      if (backend === 'indexeddb') {
        return idbRun('readonly', function (store) {
          var req = store.get(key);
          var holder = {};
          req.onsuccess = function () { holder.result = req.result ? new Uint8Array(req.result) : null; };
          return holder;
        });
      }
      return Promise.resolve(null);
    },

    /* Resolves only after the data is durably written (stream closed / transaction complete). */
    put: function (key, bytes) {
      if (backend === 'opfs') {
        return opfsDirectory()
          .then(function (dir) { return dir.getFileHandle(sanitizeKey(key), { create: true }); })
          .then(function (handle) { return handle.createWritable(); })
          .then(function (writable) {
            return writable.write(bytes).then(
              function () { return writable.close(); },
              function (err) { return writable.abort().catch(function () {}).then(function () { throw err; }); }
            );
          });
      }
      if (backend === 'indexeddb') {
        var copy = bytes.slice().buffer; // detach from the wasm heap
        return idbRun('readwrite', function (store) { store.put(copy, key); });
      }
      return Promise.reject(new DOMException('No persistent storage is available', 'NotSupportedError'));
    },

    /* { quota, usage } in bytes from navigator.storage.estimate(); zeros when unknown. */
    estimate: function () {
      if (navigator.storage && navigator.storage.estimate) {
        return navigator.storage.estimate().then(function (e) {
          return { quota: e.quota || 0, usage: e.usage || 0 };
        });
      }
      return Promise.resolve({ quota: 0, usage: 0 });
    }
  };
})();
