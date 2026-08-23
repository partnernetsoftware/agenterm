// Access to the Tauri API without a bundler.
//
// `withGlobalTauri` in tauri.conf.json publishes the API on `window.__TAURI__`,
// so the frontend stays plain ES modules served straight from `ui/` with no
// npm install, no node_modules, and no build step. This module is the single
// place that global is touched, so swapping in the npm package later is a
// one-file change.

const api = globalThis.__TAURI__;

if (!api) {
  throw new Error(
    "the Tauri API is unavailable — this page must run inside the app window",
  );
}

/** @type {(command: string, args?: Record<string, unknown>) => Promise<any>} */
export const invoke = api.core.invoke;

/** @type {(event: string, handler: (event: {payload: any}) => void) => Promise<() => void>} */
export const listen = api.event.listen;
