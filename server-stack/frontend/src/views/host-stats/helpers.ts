import type { HostMountStats, HostNetInterface, HostStatsView } from "../../api/types.js";

const MOUNT_DENY_PREFIXES = [
  "/etc/",
  "/proc/",
  "/sys/",
  "/dev/",
  "/run/",
  "/snap/",
  "cgroup",
];

/** Heuristic: hide bind-mount noise (e.g. /etc/hosts) from default mount list. */
export function filterMountsForDisplay(
  mounts: HostMountStats[],
  deployMountPath: string,
): { primary: HostMountStats[]; other: HostMountStats[] } {
  const primary: HostMountStats[] = [];
  const other: HostMountStats[] = [];
  const deployNorm = deployMountPath.replace(/\/$/, "") || "/";

  for (const m of mounts) {
    const path = m.path.replace(/\/$/, "") || "/";
    if (path === deployNorm || path === "/") {
      primary.push(m);
      continue;
    }
    if (m.total_bytes === 0) {
      other.push(m);
      continue;
    }
    const denied = MOUNT_DENY_PREFIXES.some((p) => path.startsWith(p));
    if (denied || path.includes("cgroup")) {
      other.push(m);
      continue;
    }
    if (path.startsWith("/mnt/") || path.startsWith("/var/") || path.startsWith("/home/")) {
      primary.push(m);
    } else if (path.split("/").length <= 2 && path !== "/") {
      other.push(m);
    } else {
      primary.push(m);
    }
  }

  const seen = new Set<string>();
  const uniq = (arr: HostMountStats[]) =>
    arr.filter((x) => {
      const k = x.path;
      if (seen.has(k)) {
        return false;
      }
      seen.add(k);
      return true;
    });

  return { primary: uniq(primary), other: uniq(other) };
}

export function isHiddenNetInterface(name: string): boolean {
  if (name === "lo") {
    return true;
  }
  const n = name.toLowerCase();
  if (n.startsWith("docker")) {
    return true;
  }
  if (n.startsWith("veth")) {
    return true;
  }
  if (n.startsWith("br-")) {
    return true;
  }
  if (n.startsWith("virbr")) {
    return true;
  }
  if (n.startsWith("wg")) {
    return true;
  }
  return false;
}

export function filterNetInterfaces(
  ifs: HostNetInterface[],
  showAll: boolean,
): { visible: HostNetInterface[]; hiddenCount: number } {
  if (showAll) {
    return { visible: ifs, hiddenCount: 0 };
  }
  const visible: HostNetInterface[] = [];
  let hidden = 0;
  for (const i of ifs) {
    if (isHiddenNetInterface(i.name)) {
      hidden += 1;
    } else {
      visible.push(i);
    }
  }
  return { visible, hiddenCount: hidden };
}

export type StatusLevel = "ok" | "warn" | "crit";

export function cpuStatus(pct: number): StatusLevel {
  if (pct >= 85) {
    return "crit";
  }
  if (pct >= 70) {
    return "warn";
  }
  return "ok";
}

export function pctStatus(pct: number): StatusLevel {
  if (pct >= 85) {
    return "crit";
  }
  if (pct >= 70) {
    return "warn";
  }
  return "ok";
}

export function tempStatus(c: number | null | undefined): StatusLevel {
  if (c == null || !Number.isFinite(c)) {
    return "ok";
  }
  if (c >= 90) {
    return "crit";
  }
  if (c >= 75) {
    return "warn";
  }
  return "ok";
}

export function diskUsagePct(data: HostStatsView): number {
  const t = data.disk_total_bytes;
  if (!t) {
    return 0;
  }
  return ((t - data.disk_free_bytes) / t) * 100;
}

export function memoryUsagePct(data: HostStatsView): number {
  const t = data.memory_total_bytes;
  if (!t) {
    return 0;
  }
  return (data.memory_used_bytes / t) * 100;
}
