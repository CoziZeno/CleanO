import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri expects a fixed port and must fail loudly rather than silently
// shifting to another one, because tauri.conf.json hardcodes devUrl.
export default defineConfig({
  plugins: [react()],
  resolve: {
    // Root-relative, so this file needs no Node types and stays out of the
    // app's tsconfig.
    alias: {
      "@": "/src",
    },
  },
  clearScreen: false,
  server: {
    // Bind IPv4 explicitly. Left to resolve "localhost", Vite listens on ::1
    // only while WebView2 may resolve to 127.0.0.1, and `tauri dev` then shows
    // ERR_CONNECTION_REFUSED. Pinning both sides to 127.0.0.1 removes the
    // name-resolution step entirely.
    host: "127.0.0.1",
    port: 1420,
    strictPort: true,
    watch: {
      // src-tauri is watched by cargo, not vite.
      ignored: ["**/src-tauri/**"],
    },
  },
  build: {
    // WebView2 (Win) and WKWebView (macOS 12+) both handle ES2022.
    target: "es2022",
    sourcemap: true,
  },
});
