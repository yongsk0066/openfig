import path from "node:path";
import { defineConfig, type HtmlTagDescriptor, type Plugin } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

const csp: Record<string, string> = {
  "default-src": "'self'",
  "script-src": "'self'",
  "img-src": "'self'",
  "style-src": "'self'",
  "connect-src": "'self' api:",
  "object-src": "'none'",
  "frame-src": "'none'",
};

const cspContent = Object.entries(csp)
  .map(([k, v]) => `${k} ${v}`)
  .join("; ");

const htmlCspPlugin: Plugin = {
  name: "html-csp",
  transformIndexHtml: {
    order: "post",
    handler: (_html, ctx): HtmlTagDescriptor[] => {
      if (ctx.server?.config?.mode === "development") {
        return [];
      }

      return [
        {
          injectTo: "head",
          tag: "meta",
          attrs: {
            "http-equiv": "Content-Security-Policy",
            content: cspContent,
          },
        },
      ];
    },
  },
};

// https://vitejs.dev/config/
export default defineConfig(({ command }) => ({
  plugins: [react(), tailwindcss(), htmlCspPlugin],
  server: {
    port: 3433,
  },
  build: {
    target: "esnext",
    sourcemap: command !== "build",
  },
  resolve: {
    alias: {
      "@": path.resolve(import.meta.dirname, "./src"),
      "@assets": path.resolve(import.meta.dirname, "./assets"),
    },
  },
}));
