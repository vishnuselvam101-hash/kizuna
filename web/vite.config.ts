import react from "@vitejs/plugin-react";
import { configDefaults, defineConfig } from "vitest/config";

export default defineConfig({
  plugins: [react()],
  server: { host: "127.0.0.1", proxy: { "/api": "http://127.0.0.1:8080" } },
  test: { environment: "jsdom", globals: true, exclude: [...configDefaults.exclude, "e2e/**"] },
});
