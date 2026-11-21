import { defineConfig } from "vite";

// Porta fixa: o Tauri aponta `devUrl` para ela (src-tauri/tauri.conf.json).
export default defineConfig({
  clearScreen: false,
  server: {
    port: 1430,
    strictPort: true,
    watch: { ignored: ["**/src-tauri/**"] },
  },
});
