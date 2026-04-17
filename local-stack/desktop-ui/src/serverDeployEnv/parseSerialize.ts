import { SERVER_DEPLOY_ENV_FLAT_KEYS, SERVER_DEPLOY_ENV_KNOWN_KEYS } from "./schema";

/** Parse KEY=VALUE lines (pirate-deploy.env). Сохраняем последнее значение для дубликатов. */
export function parseDotEnv(content: string): Map<string, string> {
  const m = new Map<string, string>();
  for (const line of content.split(/\r?\n/)) {
    const t = line.trim();
    if (!t || t.startsWith("#")) continue;
    const eq = t.indexOf("=");
    if (eq <= 0) continue;
    const key = t.slice(0, eq).trim();
    if (!key) continue;
    let val = t.slice(eq + 1);
    val = stripInlineComment(val);
    val = val.trim();
    val = unquote(val);
    m.set(key, val);
  }
  return m;
}

function stripInlineComment(val: string): string {
  if (val.includes("#") && !val.includes('"') && !val.includes("'")) {
    const hash = val.indexOf("#");
    if (hash >= 0) return val.slice(0, hash).trimEnd();
  }
  return val;
}

function unquote(val: string): string {
  if (val.length >= 2) {
    const a = val[0];
    const b = val[val.length - 1];
    if ((a === '"' && b === '"') || (a === "'" && b === "'")) {
      return val.slice(1, -1).replace(/\\n/g, "\n");
    }
  }
  return val;
}

export function envTruthy(s: string | undefined): boolean {
  const t = (s ?? "").trim().toLowerCase();
  return t === "1" || t === "true" || t === "yes" || t === "y" || t === "on";
}

export function boolToEnv(b: boolean): string {
  return b ? "1" : "0";
}

/** Экранирование значения в строке .env */
export function escapeEnvValue(v: string): string {
  if (v === "") return '""';
  if (/[\s#"'\\]/.test(v) || v.includes("\n")) {
    const escaped = v.replace(/\\/g, "\\\\").replace(/"/g, '\\"').replace(/\n/g, "\\n");
    return `"${escaped}"`;
  }
  return v;
}

/** Сериализация: сначала известные ключи по порядку схемы (если есть в map), затем неизвестные по алфавиту */
export function serializeDotEnv(map: Map<string, string>): string {
  const lines: string[] = [
    "# Отредактировано в Pirate Desktop. Комментарии из исходного файла не сохраняются.",
    "",
  ];
  for (const key of SERVER_DEPLOY_ENV_FLAT_KEYS) {
    if (!map.has(key)) continue;
    lines.push(`${key}=${escapeEnvValue(map.get(key) ?? "")}`);
  }
  const unknown = [...map.keys()]
    .filter((k) => !SERVER_DEPLOY_ENV_KNOWN_KEYS.has(k))
    .sort();
  if (unknown.length) {
    lines.push("");
    lines.push("# Дополнительные переменные");
    for (const key of unknown) {
      lines.push(`${key}=${escapeEnvValue(map.get(key) ?? "")}`);
    }
  }
  return lines.join("\n").trimEnd() + "\n";
}
