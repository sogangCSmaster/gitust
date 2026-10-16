import { fileURLToPath, URL } from "node:url";
import { defineConfig } from "vite";
import solidPlugin from "vite-plugin-solid";

export default defineConfig({
  plugins: [solidPlugin()],
  resolve: { alias: { "@": fileURLToPath(new URL("./src", import.meta.url)) } },
  server: {
    host: "127.0.0.1",
    port: 4200,
    strictPort: false,
    proxy: {
      "/api": "http://127.0.0.1:3000",
      "/mcp": "http://127.0.0.1:3000",
      "/oauth": "http://127.0.0.1:3000",
      "/.well-known": "http://127.0.0.1:3000",
      "/git": "http://127.0.0.1:3000",
    },
  },
  build: {
    target: "esnext",
    rollupOptions: {
      output: {
        entryFileNames: "assets/index.js",
        chunkFileNames: "assets/[name].js",
        assetFileNames: "assets/[name][extname]",
      },
    },
  },
});
