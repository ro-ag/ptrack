import { defineConfig } from "vite";

export default defineConfig({
  build: {
    assetsDir: "",
    emptyOutDir: true,
    rollupOptions: {
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
