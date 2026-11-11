// Real Rust core via tauri-specta bindings. Only the commands that exist in
// the core are wired here; everything else still goes through mock-backend
// until its milestone lands.
import { useQuery } from "@tanstack/react-query";
import { type AppError, commands } from "@/bindings";
import { isTauri } from "./platform";

/** Error thrown for a failed command; `code` is stable (SPEC §6.2). */
export class CoreError extends Error {
  constructor(
    readonly code: AppError["code"],
    message: string,
  ) {
    super(message);
  }
}

/** Unwraps a tauri-specta result, throwing `CoreError` on failure. */
export async function call<T>(
  p: Promise<{ status: "ok"; data: T } | { status: "error"; error: AppError }>,
): Promise<T> {
  const res = await p;
  if (res.status === "error") throw new CoreError(res.error.code, res.error.message);
  return res.data;
}

export function useAppInfo() {
  return useQuery({
    queryKey: ["appInfo"],
    enabled: isTauri(),
    queryFn: () => call(commands.ping()),
  });
}

/** User-facing text for error codes the UI can act on. */
export function describeError(e: unknown): string {
  if (e instanceof CoreError) {
    switch (e.code) {
      case "already_recording":
        return "A recording is already running.";
      case "not_recording":
        return "Nothing is being recorded.";
      case "audio_unavailable":
        return `Audio could not be captured: ${e.message}`;
      case "storage_failed":
        return `Saving failed: ${e.message}`;
      default:
        return e.message;
    }
  }
  return e instanceof Error ? e.message : String(e);
}
