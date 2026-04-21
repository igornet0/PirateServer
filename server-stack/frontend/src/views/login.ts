import type { ControlApiBaseValidationError } from "../api/api-base.js";
import {
  apiUrl,
  clearDesktopSetupDone,
  controlApiBaseForDesktopSetupField,
  getDesktopDisplayStreamIntent,
  getDesktopPrefillUsername,
  getDesktopSetupDomain,
  isDeployDesktopApp,
  markDesktopSetupDone,
  needsDesktopFirstRunSetup,
  parseAndValidateControlApiBase,
  setControlApiBaseOverride,
  setDesktopDisplayStreamIntent,
  setDesktopPrefillUsername,
  setDesktopSetupDomain,
} from "../api/api-base.js";
import { deployFetch } from "../api/deploy-fetch.js";
import { applyDocumentTranslations, t } from "../i18n/index.js";

const TOKEN_KEY = "deploy.accessToken";
const STATIC_DASH_KEY = "deploy.staticDashboard";
/** Optional bearer from login page "Skip" — applied once to `#api-token` on dashboard load. */
const PENDING_API_TOKEN_KEY = "deploy.pendingApiToken";

/** Clean URL for the sign-in page (dev server + nginx). */
export const LOGIN_PATH = "/login";
/** Dashboard root (serves `index.html`). */
export const DASHBOARD_PATH = "/";

export function isLoginPath(): boolean {
  try {
    const p = window.location.pathname;
    return (
      p === LOGIN_PATH ||
      p === "/login.html" ||
      p.endsWith("/login.html")
    );
  } catch {
    return false;
  }
}

/**
 * When control-api returns 401 and the user is not on the login screen, clear session and go to `/login`.
 * No-op on the login page (e.g. wrong password).
 * @returns true if a redirect was triggered (caller should not parse the body as a normal API error).
 */
export function redirectIfUnauthorized(status: number): boolean {
  if (status !== 401) {
    return false;
  }
  if (isLoginPath()) {
    return false;
  }
  clearSessionAndReload();
  return true;
}

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
 * Dashboard entry: redirect to `/login` if there is no JWT session and no "skip" mode.
 * Returns false when a redirect is in progress.
 */
export function assertDashboardAccess(): boolean {
  if (hasSessionAccess()) {
    return true;
  }
  window.location.replace(LOGIN_PATH);
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
  window.location.replace(LOGIN_PATH);
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

/** Same rules as server-stack/deploy/ubuntu/install.sh validate_domain (optional field). */
function validateOptionalDomain(d: string): boolean {
  const s = d.trim();
  if (!s) {
    return true;
  }
  if (s.length > 253 || s.includes("/") || s.includes(":") || s.includes(" ") || s.includes("..")) {
    return false;
  }
  return /^[A-Za-z0-9]([A-Za-z0-9.-]{0,251}[A-Za-z0-9])?$/.test(s);
}

/** install.sh --ui dashboard user pattern. */
function validateDashboardUsername(u: string): boolean {
  const t = u.trim();
  if (!t || t.length > 64) {
    return false;
  }
  return /^[A-Za-z0-9._-]+$/.test(t);
}

function desktopSetupErrKey(
  err: ControlApiBaseValidationError,
): "login.desktopSetupErr.empty" | "login.desktopSetupErr.invalid" | "login.desktopSetupErr.scheme" {
  switch (err) {
    case "empty":
      return "login.desktopSetupErr.empty";
    case "invalid":
      return "login.desktopSetupErr.invalid";
    case "scheme":
      return "login.desktopSetupErr.scheme";
  }
}

function populateDesktopSetupFields(): void {
  const urlEl = document.getElementById("deploy-control-api-url") as HTMLInputElement | null;
  const domainEl = document.getElementById("deploy-setup-domain") as HTMLInputElement | null;
  const userEl = document.getElementById("deploy-setup-username") as HTMLInputElement | null;
  const passEl = document.getElementById("deploy-setup-password") as HTMLInputElement | null;
  const dsEl = document.getElementById("deploy-setup-display-stream") as HTMLInputElement | null;
  if (urlEl) {
    urlEl.value = controlApiBaseForDesktopSetupField();
  }
  if (domainEl) {
    domainEl.value = getDesktopSetupDomain();
  }
  if (userEl) {
    userEl.value = getDesktopPrefillUsername();
  }
  if (passEl) {
    passEl.value = "";
  }
  if (dsEl) {
    dsEl.checked = getDesktopDisplayStreamIntent();
  }
}

function applyDesktopLoginLayout(): void {
  const setup = document.getElementById("desktop-setup");
  const main = document.getElementById("login-main-block");
  const changeWrap = document.getElementById("login-change-server-url-wrap");
  if (!setup || !main) {
    return;
  }
  if (!isDeployDesktopApp()) {
    setup.hidden = true;
    main.hidden = false;
    if (changeWrap) {
      changeWrap.hidden = true;
    }
    return;
  }
  if (needsDesktopFirstRunSetup()) {
    setup.hidden = false;
    main.hidden = true;
    if (changeWrap) {
      changeWrap.hidden = true;
    }
    populateDesktopSetupFields();
  } else {
    setup.hidden = true;
    main.hidden = false;
    if (changeWrap) {
      changeWrap.hidden = false;
    }
  }
}

function showDesktopSetupEditor(): void {
  const setup = document.getElementById("desktop-setup");
  const main = document.getElementById("login-main-block");
  const changeWrap = document.getElementById("login-change-server-url-wrap");
  if (!setup || !main) {
    return;
  }
  clearDesktopSetupDone();
  setup.hidden = false;
  main.hidden = true;
  if (changeWrap) {
    changeWrap.hidden = true;
  }
  populateDesktopSetupFields();
  const errEl = document.getElementById("desktop-setup-error");
  if (errEl) {
    errEl.textContent = "";
  }
  applyDocumentTranslations();
}

function prefillLoginFromSetup(): void {
  const su = document.getElementById("deploy-setup-username") as HTMLInputElement | null;
  const sp = document.getElementById("deploy-setup-password") as HTMLInputElement | null;
  const uEl = document.getElementById("login-username") as HTMLInputElement | null;
  const pEl = document.getElementById("login-password") as HTMLInputElement | null;
  if (uEl && su) {
    uEl.value = su.value.trim();
  }
  if (pEl && sp) {
    pEl.value = sp.value;
  }
}

/**
 * Login page: redirect to dashboard if already authenticated; otherwise bind form and skip.
 */
export function initLoginPage(): void {
  if (hasSessionAccess()) {
    window.location.replace(DASHBOARD_PATH);
    return;
  }

  applyDesktopLoginLayout();

  const setupErr = document.getElementById("desktop-setup-error");
  const domainEl = document.getElementById("deploy-setup-domain") as HTMLInputElement | null;
  const urlEl = document.getElementById("deploy-control-api-url") as HTMLInputElement | null;

  domainEl?.addEventListener("blur", () => {
    const d = domainEl.value.trim();
    const u = urlEl?.value?.trim() ?? "";
    if (d && validateOptionalDomain(d) && !u) {
      if (urlEl) {
        urlEl.value = `https://${d}`;
      }
    }
  });

  const setupContinue = document.getElementById("desktop-setup-continue");
  setupContinue?.addEventListener("click", () => {
    const domainRaw = (document.getElementById("deploy-setup-domain") as HTMLInputElement | null)
      ?.value ?? "";
    const userRaw = (document.getElementById("deploy-setup-username") as HTMLInputElement | null)
      ?.value ?? "";
    const passRaw =
      (document.getElementById("deploy-setup-password") as HTMLInputElement | null)?.value ?? "";
    let urlRaw = (document.getElementById("deploy-control-api-url") as HTMLInputElement | null)
      ?.value ?? "";
    const ds = (document.getElementById("deploy-setup-display-stream") as HTMLInputElement | null)
      ?.checked ?? false;

    if (!validateOptionalDomain(domainRaw)) {
      if (setupErr) {
        setupErr.textContent = t("login.err.desktopSetupDomain");
      }
      return;
    }

    const domainTrim = domainRaw.trim();
    urlRaw = urlRaw.trim();
    if (!urlRaw && domainTrim) {
      urlRaw = `https://${domainTrim}`;
      if (urlEl) {
        urlEl.value = urlRaw;
      }
    }

    const v = parseAndValidateControlApiBase(urlRaw);
    if (!v.ok) {
      if (setupErr) {
        setupErr.textContent = t(desktopSetupErrKey(v.error));
      }
      return;
    }

    if (!validateDashboardUsername(userRaw)) {
      if (setupErr) {
        setupErr.textContent = t("login.err.desktopSetupUsername");
      }
      return;
    }
    if (!passRaw) {
      if (setupErr) {
        setupErr.textContent = t("login.err.desktopSetupPassword");
      }
      return;
    }

    if (setupErr) {
      setupErr.textContent = "";
    }

    setDesktopSetupDomain(domainTrim);
    setControlApiBaseOverride(v.base);
    setDesktopPrefillUsername(userRaw.trim());
    setDesktopDisplayStreamIntent(ds);
    markDesktopSetupDone();
    applyDesktopLoginLayout();
    prefillLoginFromSetup();
    applyDocumentTranslations();
  });

  document.getElementById("login-change-server-url")?.addEventListener("click", () => {
    showDesktopSetupEditor();
  });

  const form = document.getElementById("login-form") as HTMLFormElement | null;
  const errEl = document.getElementById("login-error");
  const advToggle = document.getElementById("login-advanced-toggle");
  const adv = document.getElementById("login-advanced");

  const goToDashboard = () => {
    window.location.replace(DASHBOARD_PATH);
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
      const loginUrl = apiUrl("/api/v1/auth/login");
      try {
        const r = await deployFetch(loginUrl, {
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
          const msg = e instanceof Error ? e.message : String(e);
          errEl.textContent =
            msg.includes("expected pattern") || msg.includes("URL")
              ? t("login.err.network")
              : msg || t("login.err.network");
        }
      }
    })();
  });

  advToggle?.addEventListener("click", () => {
    if (adv) adv.hidden = !adv.hidden;
  });
}
