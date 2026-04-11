import type {
  CpuDetail,
  DiskDetail,
  HostStatsDetailKind,
  HostStatsView,
  MemoryDetail,
  NetworkDetail,
  ProcessesDetail,
} from "./types.js";
import {
  fetchHostStats,
  fetchHostStatsDetail,
  fetchHostStatsSeries,
} from "./client.js";

/** Fetches host metrics from control-api (`GET /api/v1/host-stats`). */
export class HostStatsClient {
  fetch(): Promise<HostStatsView> {
    return fetchHostStats();
  }

  fetchDetail(
    kind: HostStatsDetailKind,
    params?: Record<string, string | number | undefined>,
  ): Promise<
    CpuDetail | MemoryDetail | DiskDetail | NetworkDetail | ProcessesDetail
  > {
    return fetchHostStatsDetail(
      kind as never,
      params,
    ) as Promise<
      CpuDetail | MemoryDetail | DiskDetail | NetworkDetail | ProcessesDetail
    >;
  }

  fetchSeries(metric: string, range?: string) {
    return fetchHostStatsSeries(metric, range);
  }
}
