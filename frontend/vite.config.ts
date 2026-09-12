import { defineConfig } from "vite";

export default defineConfig({
  build: {
    // Preserve the Vite 7 browser floor when upgrading the bundler.
    target: ["chrome107", "edge107", "firefox104", "safari16"],
    assetsDir: "",
    emptyOutDir: true,
    rolldownOptions: {
      output: {
        entryFileNames: "app.js",
        assetFileNames: (assetInfo) =>
          assetInfo.names.some((name) => name.endsWith(".css"))
            ? "style.css"
            : "[name][extname]",
      },
    },
  },
  test: {
    include: ["src/**/*.test.{js,ts}"],
  },
});
