import { createSarmgReactViteConfig } from "@sarmg/admin-web/vite";
import { mergeConfig } from "vite";

export default mergeConfig(createSarmgReactViteConfig({ base: "/admin/" }), {
  build: {
    rollupOptions: {
      output: {
        entryFileNames: "assets/admin.js",
        chunkFileNames: "assets/[name].js",
        assetFileNames: (assetInfo: { names: string[] }) =>
          assetInfo.names.some((name) => name.endsWith(".css"))
            ? "assets/admin.css"
            : "assets/[name][extname]",
      },
    },
  },
  server: {
    port: 5174,
    proxy: { "/api/v2": "http://127.0.0.1:8080" },
  },
});
