import { useQuery, type UseQueryOptions, type UseQueryResult } from "@tanstack/react-query";
import { invoke, isTauri } from "@tauri-apps/api/core";

/**
 * Cached Tauri `invoke` with TanStack Query (dedupe + staleTime for server-backed reads).
 */
export function useTauriQuery<T>(
  command: string,
  args?: Record<string, unknown>,
  options?: Omit<UseQueryOptions<T, Error>, "queryKey" | "queryFn">,
): UseQueryResult<T, Error> {
  return useQuery({
    queryKey: [command, args ?? {}],
    enabled: (options?.enabled ?? true) && isTauri(),
    queryFn: () => invoke<T>(command, args),
    staleTime: 30_000,
    ...options,
  });
}
