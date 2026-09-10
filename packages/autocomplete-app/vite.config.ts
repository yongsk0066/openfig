import { defineConfig, loadEnv, type Plugin, type HtmlTagDescriptor } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

const csp: Record<string, string> = {
  "default-src": "'self'",
  // blob: is needed for loading dev specs
  "script-src": "'self' spec: blob:",
  "style-src": "'self' spec:",
  "img-src": "'self' data: fig: icon:",
  "connect-src": "'self' spec: api:",
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
export default defineConfig(({ mode }) => {
  process.env = { ...process.env, ...loadEnv(mode, process.cwd(), "") };

  return {
    plugins: [react(), tailwindcss(), htmlCspPlugin],
    server: {
      port: process.env.PORT ? parseInt(process.env.PORT, 10) : 3124,
      strictPort: true,
      warmup: {
        clientFiles: ["./src/main.tsx", "./src/App.tsx", "./src/components/Suggestion.tsx"],
      },
    },
    build: {
      target: "esnext",
      // TODO: re-enable prod source maps to upload them to sentry (see build CIs)
      sourcemap: false,
      // The suggestion list is one screen of a local `figresource://` page, so a
      // single chunk is what we want; the limit is here to keep the warning
      // meaningful rather than permanent.
      chunkSizeWarningLimit: 700,
      rolldownOptions: {
        external: ["?type=option", "?type=carrot", "?type=command", "?type=box"],
      },
    },
    define: {
      // Some dependencies still read `process.env.*`; there is no `process` in
      // the webview.
      "process.env": {},
    },
  };
});
