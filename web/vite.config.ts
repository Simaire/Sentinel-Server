import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

// L'interface est servie par le serveur sous …/admin/, à la racine ou sous un sous-chemin
// (PUBLIC_URL) : chemins relatifs. En développement, /api et /admin/login sont relayés vers
// le serveur local.
export default defineConfig({
  base: "./",
  plugins: [svelte()],
  build: { outDir: "dist", emptyOutDir: true, sourcemap: false },
  server: {
    port: 5180,
    proxy: {
      "/api": "http://127.0.0.1:8080",
      "/admin/login": "http://127.0.0.1:8080"
    }
  }
});
