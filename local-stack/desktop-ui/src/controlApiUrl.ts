/**
 * Best-effort control-api HTTP base from a deploy-server gRPC URL.
 * Prefer the same host without an explicit port (nginx / TLS on :80 or :443).
 * Use an explicit `:8080` only when control-api listens on all interfaces without a reverse proxy
 * (see `DEPLOY_CONTROL_API_PUBLIC_URL` and `CONTROL_API_BIND` in server `env.example`).
 */
export function suggestControlApiFromGrpcUrl(endpoint: string): string | null {
  const s = endpoint.trim();
  if (!s) return null;
  const isHttps = /^https:\/\//i.test(s);
  const scheme = isHttps ? "https" : "http";

  let authority = s
    .replace(/^https:\/\//i, "")
    .replace(/^http:\/\//i, "");
  const slash = authority.indexOf("/");
  if (slash >= 0) authority = authority.slice(0, slash);
  authority = authority.trim();
  if (!authority) return null;

  if (authority.endsWith(":50051")) {
    authority = authority.slice(0, authority.length - ":50051".length);
  } else if (authority.startsWith("[")) {
    const close = authority.indexOf("]");
    if (close > 0) {
      const tail = authority.slice(close + 1);
      if (tail.startsWith(":")) {
        authority = authority.slice(0, close + 1);
      }
    }
  } else {
    const colon = authority.lastIndexOf(":");
    if (colon > 0 && authority.indexOf(":") === colon) {
      authority = authority.slice(0, colon);
    }
  }

  return `${scheme}://${authority}`;
}
