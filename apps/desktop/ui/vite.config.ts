import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

// Tauri expects a fixed dev port; the same bundle serves main / tray / mini windows.
export default defineConfig({
  plugins: [svelte()],
  clearScreen: false,
  server: { port: 1420, strictPort: true, host: "127.0.0.1", hmr: { overlay: false }, watch: { ignored: ["**/src-tauri/gen/**"] }, fs: { allow: ["../.."] } },
  build: { target: "safari15", sourcemap: false, chunkSizeWarningLimit: 400 },
});
