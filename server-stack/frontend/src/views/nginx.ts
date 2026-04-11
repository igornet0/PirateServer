import { fetchNginxConfig, putNginxConfig } from "../api/client.js";
import { ApiRequestError } from "../api/types.js";
import { t } from "../i18n/index.js";

function formatErr(e: unknown): string {
  if (e instanceof ApiRequestError) {
    return `${e.message} (${e.status}${e.code ? ` / ${e.code}` : ""})`;
  }
  return String(e);
}

export async function loadNginx(): Promise<void> {
  const editor = document.getElementById("nginx-editor") as HTMLTextAreaElement;
  const result = document.getElementById("nginx-result")!;
  result.textContent = "";
  try {
    const data = await fetchNginxConfig();
    if (!data.enabled) {
      editor.value = "";
      editor.placeholder = t("nginx.disabled.placeholder");
      result.textContent = t("nginx.disabled.apiMsg");
      return;
    }
    editor.value = data.content ?? "";
    editor.placeholder = "";
    result.textContent = t("nginx.loadedPattern", { path: data.path ?? "" });
  } catch (e) {
    result.textContent = formatErr(e);
  }
}

export async function saveNginx(): Promise<void> {
  const editor = document.getElementById("nginx-editor") as HTMLTextAreaElement;
  const result = document.getElementById("nginx-result")!;
  result.textContent = t("nginx.saving");
  try {
    const raw = await putNginxConfig(editor.value);
    result.textContent = JSON.stringify(raw, null, 2);
  } catch (e) {
    result.textContent = formatErr(e);
  }
}
