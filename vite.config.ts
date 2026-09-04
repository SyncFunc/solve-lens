import { defineConfig } from "vite";
import { resolve } from "node:path";

export default defineConfig({
  clearScreen: false,
  build: { rollupOptions: { input: { index: resolve(__dirname, "index.html"), mobile: resolve(__dirname, "mobile.html") } } },
  server: { port: 1420, strictPort: true },
  envPrefix: ["VITE_", "TAURI_"]
});
