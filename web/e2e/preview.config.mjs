// Serves the production build (web/dist) for the browser acceptance run; /api goes to the real kizuna-server.
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

export default defineConfig({
  root: new URL("..", import.meta.url).pathname,
  plugins: [react()],
  preview: {
    host: "127.0.0.1",
    port: Number(process.env.WEB_PORT),
    strictPort: true,
    proxy: { "/api": { target: process.env.KIZUNA_API, xfwd: true } },
  },
});
