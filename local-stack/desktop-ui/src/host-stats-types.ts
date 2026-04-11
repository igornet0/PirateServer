/** Matches JSON from `fetch_host_stats_json` / deploy `HostStatsResponse` map. */

export type HostMountStats = {
  path: string;
  total_bytes: number;
  free_bytes: number;
};

export type HostNetInterface = {
  name: string;
  rx_bytes_per_s: number;
  tx_bytes_per_s: number;
  rx_errors: number;
  tx_errors: number;
};

export type HostLogLine = {
  ts_ms: number;
  level: string;
  message: string;
};

export type HostStatsSnapshot = {
  disk_free_bytes: number;
  disk_total_bytes: number;
  disk_mount_path: string;
  memory_used_bytes: number;
  memory_total_bytes: number;
  cpu_usage_percent: number;
  load_average_1m: number;
  load_average_5m: number;
  load_average_15m: number;
  temperature_current_celsius?: number | null;
  temperature_avg_celsius?: number | null;
  process_count: number;
  disk_mounts: HostMountStats[];
  network_interfaces: HostNetInterface[];
  log_tail: HostLogLine[];
};

/** Proto `HostStatsDetailKind` (non-zero). */
export const HOST_STATS_DETAIL_KIND = {
  CPU: 1,
  MEMORY: 2,
  DISK: 3,
  NETWORK: 4,
  PROCESSES: 5,
} as const;

export function parseHostStatsSnapshot(raw: string): HostStatsSnapshot {
  const j = JSON.parse(raw) as Record<string, unknown>;
  const arr = <T>(v: unknown): T[] => (Array.isArray(v) ? (v as T[]) : []);

  return {
    disk_free_bytes: Number(j.disk_free_bytes) || 0,
    disk_total_bytes: Number(j.disk_total_bytes) || 0,
    disk_mount_path: String(j.disk_mount_path ?? ""),
    memory_used_bytes: Number(j.memory_used_bytes) || 0,
    memory_total_bytes: Number(j.memory_total_bytes) || 0,
    cpu_usage_percent: Number(j.cpu_usage_percent) || 0,
    load_average_1m: Number(j.load_average_1m) || 0,
    load_average_5m: Number(j.load_average_5m) || 0,
    load_average_15m: Number(j.load_average_15m) || 0,
    temperature_current_celsius:
      j.temperature_current_celsius === undefined || j.temperature_current_celsius === null
        ? null
        : Number(j.temperature_current_celsius),
    temperature_avg_celsius:
      j.temperature_avg_celsius === undefined || j.temperature_avg_celsius === null
        ? null
        : Number(j.temperature_avg_celsius),
    process_count: Number(j.process_count) || 0,
    disk_mounts: arr<HostMountStats>(j.disk_mounts),
    network_interfaces: arr<HostNetInterface>(j.network_interfaces),
    log_tail: arr<HostLogLine>(j.log_tail),
  };
}

export const MOCK_HOST_STATS: HostStatsSnapshot = parseHostStatsSnapshot(
  JSON.stringify({
    disk_free_bytes: 116.9 * 1024 * 1024 * 1024,
    disk_total_bytes: 125.64 * 1024 * 1024 * 1024,
    disk_mount_path: "/",
    memory_used_bytes: 211.84 * 1024 * 1024,
    memory_total_bytes: 950.68 * 1024 * 1024,
    cpu_usage_percent: 6.8,
    load_average_1m: 0.23,
    load_average_5m: 0.11,
    load_average_15m: 0.08,
    temperature_current_celsius: 38.9,
    temperature_avg_celsius: 38.9,
    process_count: 220,
    disk_mounts: [],
    network_interfaces: [],
    log_tail: [],
  }),
);
