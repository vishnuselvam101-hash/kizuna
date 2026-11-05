// Browser acceptance suite (scripts/e2e-browser.sh starts the real server, DB and built web app first).
// EN journeys run in tenant e2e-en at 1440 px dark; JA journeys in tenant e2e-ja at 390 px light. The matrix
// project then visits every page state in both languages at 1440/390 × dark/light (axe + screenshots).
import { defineConfig, devices } from "@playwright/test";

const base = process.env.E2E_BASE_URL ?? "http://localhost:5173";
const chromium = { ...devices["Desktop Chrome"], channel: undefined };

export default defineConfig({
  testDir: ".",
  outputDir: "results/artifacts",
  workers: 1,
  fullyParallel: false,
  retries: 0,
  timeout: 120_000,
  expect: { timeout: 15_000 },
  globalSetup: "./global-setup.ts",
  reporter: [["list"], ["json", { outputFile: "results/report.json" }]],
  use: {
    baseURL: base,
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
    acceptDownloads: true,
    actionTimeout: 20_000,
  },
  projects: [
    {
      name: "en",
      testMatch: /journeys\/.*\.spec\.ts/,
      metadata: { lang: "en", theme: "dark" },
      use: { ...chromium, locale: "en-GB", viewport: { width: 1440, height: 900 } },
    },
    {
      name: "ja",
      testMatch: /journeys\/.*\.spec\.ts/,
      metadata: { lang: "ja", theme: "light" },
      use: { ...chromium, locale: "ja-JP", viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true, deviceScaleFactor: 2 },
    },
    {
      name: "keyboard",
      testMatch: /keyboard\.spec\.ts/,
      metadata: { lang: "en", theme: "dark" },
      use: { ...chromium, locale: "en-GB", viewport: { width: 1440, height: 900 } },
    },
    {
      name: "matrix",
      testMatch: /matrix\.spec\.ts/,
      metadata: { lang: "en", theme: "dark" },
      use: { ...chromium },
    },
  ],
});
