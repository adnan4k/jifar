import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect } from "react";
import { commands } from "@/bindings";
import { backend } from "./mock-backend";
import { call } from "./native";
import { isTauri } from "./platform";
import type { AudioDevice } from "./types";

export const queryKeys = {
  meetings: (q: string) => ["meetings", q] as const,
  meeting: (id: string) => ["meeting", id] as const,
  devices: ["devices"] as const,
};

/** Stand-in for Rust → UI events (`meeting://status`, `transcript://segments`). */
export function useBackendEvents() {
  const qc = useQueryClient();
  useEffect(() => backend.subscribe(() => void qc.invalidateQueries()), [qc]);
}

export function useMeetings(query: string) {
  return useQuery({
    queryKey: queryKeys.meetings(query),
    queryFn: () => backend.meetingsList(query),
    placeholderData: (prev) => prev,
  });
}

export function useMeeting(id: string | null) {
  return useQuery({
    queryKey: queryKeys.meeting(id ?? ""),
    queryFn: () => backend.meetingGet(id ?? ""),
    enabled: id !== null,
  });
}

/** Real devices from the core in the app, mock devices in the browser. */
export function useDevices() {
  return useQuery({
    queryKey: queryKeys.devices,
    queryFn: async (): Promise<{ inputs: AudioDevice[]; outputs: AudioDevice[] }> =>
      isTauri() ? call(commands.devicesList()) : backend.devicesList(),
  });
}

export function useGenerateNotes() {
  return useMutation({ mutationFn: (id: string) => backend.notesGenerate(id) });
}

export function useRetryQueue() {
  return useMutation({ mutationFn: (id: string) => backend.queueRetry(id) });
}
