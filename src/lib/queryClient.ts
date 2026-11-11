import { QueryClient } from "@tanstack/react-query";

/** Shared so stores and event listeners can refresh cached queries. */
export const queryClient = new QueryClient({
  defaultOptions: { queries: { staleTime: Number.POSITIVE_INFINITY, retry: false } },
});
