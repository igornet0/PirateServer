import type { Plugin } from "vite";

/** Dev / preview: serve `login.html` at `/login` (clean URL). */
export function viteLoginRoutePlugin(): Plugin {
  return {
    name: "pirate-login-route",
    enforce: "pre",
    configureServer(server) {
      server.middlewares.use((req, _res, next) => {
        const raw = req.url;
        if (!raw) {
          next();
          return;
        }
        const q = raw.indexOf("?");
        const pathPart = q >= 0 ? raw.slice(0, q) : raw;
        if (pathPart === "/login" || pathPart === "/login/") {
          req.url = "/login.html" + (q >= 0 ? raw.slice(q) : "");
        }
        next();
      });
    },
  };
}
