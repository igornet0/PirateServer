import { t } from "../i18n/index.js";

const TOKEN_KEY = "deploy.accessToken";
const STATIC_DASH_KEY = "deploy.staticDashboard";
/** Optional bearer from login page "Skip" — applied once to `#api-token` on dashboard load. */
const PENDING_API_TOKEN_KEY = "deploy.pendingApiToken";

export function hasSessionAccess(): boolean {
  try {
    const t = sessionStorage.getItem(TOKEN_KEY);
    if (t?.trim()) {
      return true;
    }
    if (sessionStorage.getItem(STATIC_DASH_KEY) === "1") {
      return true;
    }
  } catch {
    /* ignore */
  }
  return false;
}

/**
 * Dashboard entry: redirect to `login.html` if there is no JWT session and no "skip" mode.
 * Returns false when a redirect is in progress.
 */
export function assertDashboardAccess(): boolean {
  if (hasSessionAccess()) {
    return true;
  }
  window.location.replace("login.html");
  return false;
}

export function clearSessionAndReload(): void {
  try {
    sessionStorage.removeItem(TOKEN_KEY);
    sessionStorage.removeItem(STATIC_DASH_KEY);
    sessionStorage.removeItem(PENDING_API_TOKEN_KEY);
  } catch {
    /* ignore */
  }
  window.location.replace("login.html");
}

/** Call once on dashboard init after `assertDashboardAccess()` — copies pending token from login skip flow. */
export function applyPendingApiTokenFromLogin(): void {
  try {
    const pending = sessionStorage.getItem(PENDING_API_TOKEN_KEY);
    if (!pending?.trim()) {
      return;
    }
    const dashTok = document.getElementById("api-token") as HTMLInputElement | null;
    if (dashTok && !dashTok.value.trim()) {
      dashTok.value = pending.trim();
    }
    sessionStorage.removeItem(PENDING_API_TOKEN_KEY);
  } catch {
    /* ignore */
  }
}

/**
 * Login page: redirect to dashboard if already authenticated; otherwise bind form and skip.
 */
export function initLoginPage(): void {
  if (hasSessionAccess()) {
    window.location.replace("index.html");
    return;
  }

  const form = document.getElementById("login-form") as HTMLFormElement | null;
  const errEl = document.getElementById("login-error");
  const skipBtn = document.getElementById("login-skip");
  const advToggle = document.getElementById("login-advanced-toggle");
  const adv = document.getElementById("login-advanced");

  const goToDashboard = () => {
    window.location.replace("index.html");
  };

  form?.addEventListener("submit", (ev) => {
    ev.preventDefault();
    const u = (document.getElementById("login-username") as HTMLInputElement | null)
      ?.value?.trim();
    const p = (document.getElementById("login-password") as HTMLInputElement | null)?.value ?? "";
    if (!u) {
      if (errEl) errEl.textContent = t("login.err.username");
      return;
    }
    if (errEl) errEl.textContent = "";
    void (async () => {
      try {
        const r = await fetch("/api/v1/auth/login", {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ username: u, password: p }),
        });
        const data = (await r.json()) as {
          access_token?: string;
          error?: { message?: string };
        };
        if (!r.ok) {
          const msg =
            data.error?.message ??
            (r.status === 503
              ? t("login.err.login503")
              : t("login.err.http", { status: r.status }));
          if (errEl) errEl.textContent = msg;
          return;
        }
        if (!data.access_token) {
          if (errEl) errEl.textContent = t("login.err.invalidResponse");
          return;
        }
        try {
          sessionStorage.setItem(TOKEN_KEY, data.access_token);
        } catch {
          if (errEl) errEl.textContent = t("login.err.saveSession");
          return;
        }
        for (const id of ["login-username", "login-password", "api-token-login"]) {
          const el = document.getElementById(id) as HTMLInputElement | null;
          if (el) el.value = "";
        }
        goToDashboard();
      } catch (e) {
        if (errEl) {
          errEl.textContent =
            e instanceof Error ? e.message : t("login.err.network");
        }
      }
    })();
  });

  skipBtn?.addEventListener("click", () => {
    const copyFrom = document.getElementById("api-token-login") as HTMLInputElement | null;
    const tok = copyFrom?.value?.trim() ?? "";
    if (tok) {
      try {
        sessionStorage.setItem(PENDING_API_TOKEN_KEY, tok);
      } catch {
        /* ignore */
      }
    }
    try {
      sessionStorage.setItem(STATIC_DASH_KEY, "1");
    } catch {
      /* ignore */
    }
    goToDashboard();
  });

  advToggle?.addEventListener("click", () => {
    if (adv) adv.hidden = !adv.hidden;
  });
}
