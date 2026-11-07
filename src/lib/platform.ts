/** True when running inside the Tauri WebView (vs. `pnpm dev` in a browser). */
export const isTauri = (): boolean => "__TAURI_INTERNALS__" in window;
