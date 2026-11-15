import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";

// Tauri expects a fixed dev port and no browser auto-open.
export default defineConfig({
  plugins: [vue()],
  clearScreen: false,
  server: { port: 5173, strictPort: true, host: "127.0.0.1" },
  build: {
    target: ["es2021", "safari15"],
    outDir: "dist",
    emptyOutDir: true,
  },
});
