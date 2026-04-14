#!/usr/bin/env bash
# Lightweight GUI / desktop session probe (same heuristics as `pirate gui-check` on Linux).
# Prints one JSON line to stdout (aligned with `pirate gui-check`): gui_detected, reasons,
# monitor_count (null in shell), drm_connectors_connected from /sys/class/drm when no X11.
set -euo pipefail

REASONS=()
GUI=0

if command -v systemctl >/dev/null 2>&1; then
  _def="$(systemctl get-default 2>/dev/null || true)"
  if [[ "${_def:-}" == "graphical.target" ]]; then
    GUI=1
    REASONS+=("systemd_default_graphical")
  fi
fi

if [[ -n "${WAYLAND_DISPLAY:-}" ]]; then
  GUI=1
  REASONS+=("wayland_display")
fi
if [[ -n "${DISPLAY:-}" ]]; then
  GUI=1
  REASONS+=("display_env")
fi

if [[ -n "${XDG_SESSION_TYPE:-}" ]]; then
  _x="$(printf '%s' "${XDG_SESSION_TYPE}" | tr '[:upper:]' '[:lower:]')"
  case "$_x" in
    wayland|x11|tty)
      GUI=1
      REASONS+=("xdg_session_type_${_x}")
      ;;
  esac
fi

if command -v loginctl >/dev/null 2>&1; then
  _desk="$(loginctl show-session self -p Desktop 2>/dev/null | sed -n 's/^Desktop=//p' | head -1 | tr -d '\r\n')"
  if [[ -n "${_desk:-}" ]] && [[ "$_desk" != "(null)" ]]; then
    GUI=1
    REASONS+=("loginctl_desktop_session")
  fi
fi

# Без X11/Wayland xcap не видит мониторы; DRM sysfs показывает физически подключённые выходы.
DRM_N=0
if [[ -d /sys/class/drm ]]; then
  shopt -s nullglob
  for _st in /sys/class/drm/card*-*/status; do
    if grep -qx connected "$_st" 2>/dev/null; then
      DRM_N=$((DRM_N+1))
    fi
  done
  shopt -u nullglob
fi
if [[ "$DRM_N" -gt 0 ]]; then
  GUI=1
  REASONS+=("drm_sysfs_connected")
fi

if command -v python3 >/dev/null 2>&1; then
  GUI_FLAG=0
  [[ "$GUI" == "1" ]] && GUI_FLAG=1
  REASON_JSON="$(printf '%s\n' "${REASONS[@]}" | python3 -c 'import json,sys; print(json.dumps([l.strip() for l in sys.stdin if l.strip()]))')"
  python3 -c "import json,sys; d=int(sys.argv[3]); print(json.dumps({'gui_detected':bool(int(sys.argv[1])),'reasons':json.loads(sys.argv[2]),'monitor_count':None,'drm_connectors_connected':(d if d>0 else None)}))" "$GUI_FLAG" "$REASON_JSON" "$DRM_N"
else
  # No python: minimal JSON
  if [[ "$GUI" == "1" ]]; then
    echo '{"gui_detected":true,"reasons":["fallback"],"monitor_count":null,"drm_connectors_connected":null}'
  else
    echo '{"gui_detected":false,"reasons":[],"monitor_count":null,"drm_connectors_connected":null}'
  fi
fi
