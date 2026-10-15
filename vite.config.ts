import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// The renderer lives in ui/. Tauri loads the dev server in development and ui/dist in builds.
export default defineConfig({
  root: "ui",
  plugins: [react()],
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  build: { outDir: "dist", emptyOutDir: true, target: "safari15" },
  test: {
    // Renderer tests under ui/ (jsdom); sidecar tests opt into node with a file comment.
    root: ".",
    include: ["ui/src/**/*.test.{ts,tsx}", "sidecar/test/**/*.test.ts"],
    environment: "jsdom",
  },
});
