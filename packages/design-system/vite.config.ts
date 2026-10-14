import { defineConfig } from "vite";
import solidPlugin from "vite-plugin-solid";

export default defineConfig({
  plugins: [solidPlugin()],
  server: {
    port: 4200,
    strictPort: false,
  },
  build: {
    target: "esnext",
  },
});
