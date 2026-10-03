import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";


export default defineConfig(() => ({
  plugins: [react()],
  clearScreen: false,
  build: {
    rolldownOptions: {
      output: {
        codeSplitting: {
          groups: [{ name: "react-vendor", test: /node_modules[\\/](?:react|react-dom|scheduler)[\\/]/ }],
        },
      },
    },
  },
  server: {
    host: "127.0.0.1",
    port: 1420,
    strictPort: true,
    watch: {
      ignored: ["**/src-tauri/**", `${decodeURIComponent(new URL(".local/", import.meta.url).pathname)}**`, `${decodeURIComponent(new URL("tmp/", import.meta.url).pathname)}**`],
    },
  },
}));
