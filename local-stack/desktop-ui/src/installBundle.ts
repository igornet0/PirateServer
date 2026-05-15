/** True when pasted text is a deploy-server install JSON (token + url + pairing). */
export function looksLikeInstallBundle(raw: string): boolean {
  const t = raw.trim();
  if (!t.startsWith("{")) return false;
  try {
    const j = JSON.parse(t) as Record<string, unknown>;
    return (
      typeof j.token === "string" &&
      j.token.length > 0 &&
      typeof j.url === "string" &&
      j.url.length > 0 &&
      typeof j.pairing === "string" &&
      j.pairing.length > 0
    );
  } catch {
    return false;
  }
}
