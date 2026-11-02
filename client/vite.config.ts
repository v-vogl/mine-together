import { defineConfig } from "vite";

// In development the page comes from Vite and /ws is passed on to the Rust server.
export default defineConfig({
  server: {
    host: true,
    proxy: {
      "/ws": { target: "ws://localhost:8080", ws: true },
    },
  },
});
