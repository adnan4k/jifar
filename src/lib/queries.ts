import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect } from "react";
import { backend } from "./mock-backend";

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

export function useDevices() {
  return useQuery({ queryKey: queryKeys.devices, queryFn: () => backend.devicesList() });
}

export function useGenerateNotes() {
  return useMutation({ mutationFn: (id: string) => backend.notesGenerate(id) });
}

export function useRetryQueue() {
  return useMutation({ mutationFn: (id: string) => backend.queueRetry(id) });
}
