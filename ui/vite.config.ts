import { defineConfig } from "vite";

// Tauri loads the built UI from a custom protocol (tauri://localhost), not a
// real HTTP origin. Absolute asset paths like `/assets/foo.js` resolve against
// the protocol root incorrectly and fail to load — leaving a blank white
// window (HTML shell only, no CSS/JS). Relative paths (`./assets/...`) work.
export default defineConfig({
  base: "./",
  clearScreen: false,
  server: {
    port: 5173,
    strictPort: true,
  },
  build: {
    // Keep dist predictable for tauri.conf.json frontendDist.
    outDir: "dist",
    emptyOutDir: true,
  },
});
