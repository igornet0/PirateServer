/**
 * Best-effort control-api HTTP base from a deploy-server gRPC URL.
 * Standard gRPC port `:50051` is mapped to control-api `:8080` (same as server `derive_control_api_url_from_grpc`).
 * Other authorities without a port stay host-only (nginx on :80 / :443).
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

  let strippedStandardGrpc = false;
  if (authority.endsWith(":50051")) {
    authority = authority.slice(0, authority.length - ":50051".length);
    strippedStandardGrpc = true;
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

  if (strippedStandardGrpc) {
    return `${scheme}://${authority}:8080`;
  }
  return `${scheme}://${authority}`;
}
