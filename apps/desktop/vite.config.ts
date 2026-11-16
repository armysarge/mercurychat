import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";
import VueI18nPlugin from "@intlify/unplugin-vue-i18n/vite";

// Tauri expects a fixed dev port and no browser auto-open.
export default defineConfig({
  plugins: [
    vue(),
    // The message files are compiled to functions at build time, and the
    // message compiler is left out of the bundle: Tauri's CSP forbids eval, so
    // nothing may be compiled in the window. A message that was not
    // precompiled fails loudly instead of being compiled with new Function.
    VueI18nPlugin({
      include: [fileURLToPath(new URL("./src/i18n/locales/**", import.meta.url))],
      compositionOnly: true,
      runtimeOnly: true,
      dropMessageCompiler: true,
      strictMessage: true,
    }),
  ],
  clearScreen: false,
  server: { port: 5173, strictPort: true, host: "127.0.0.1" },
  build: {
    target: ["es2021", "safari15"],
    outDir: "dist",
    emptyOutDir: true,
  },
});
