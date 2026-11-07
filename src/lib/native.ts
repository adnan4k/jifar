// Real Rust core via tauri-specta bindings. Only the commands that exist in
// the core are wired here; everything else still goes through mock-backend
// until its milestone lands.
import { useQuery } from "@tanstack/react-query";
import { commands } from "@/bindings";
import { isTauri } from "./platform";

export function useAppInfo() {
  return useQuery({
    queryKey: ["appInfo"],
    enabled: isTauri(),
    queryFn: async () => {
      const res = await commands.ping();
      if (res.status === "error") throw new Error(`${res.error.code}: ${res.error.message}`);
      return res.data;
    },
  });
}
