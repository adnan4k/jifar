import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect } from "react";
import { commands, type Meeting, type MeetingDetailDto } from "@/bindings";
import { backend } from "./mock-backend";
import { call } from "./native";
import { isTauri } from "./platform";
import type { AudioDevice, MeetingDetail, MeetingSummary } from "./types";

export const queryKeys = {
  meetings: (q: string) => ["meetings", q] as const,
  meeting: (id: string) => ["meeting", id] as const,
  devices: ["devices"] as const,
};

/** Browser preview only: the mock backend's change notifications. In the
 * app, core events refresh queries instead (see `connectRecording`). */
export function useBackendEvents() {
  const qc = useQueryClient();
  useEffect(() => {
    if (isTauri()) return;
    return backend.subscribe(() => void qc.invalidateQueries());
  }, [qc]);
}

function toSummary(m: Meeting): MeetingSummary {
  return {
    id: m.id,
    title: m.title,
    language: m.language,
    status: m.status,
    startedAt: m.startedAt ?? 0,
    durationMs: m.durationMs ?? 0,
  };
}

function toDetail(d: MeetingDetailDto): MeetingDetail {
  return {
    ...toSummary(d.meeting),
    segments: d.segments.map((s) => ({
      id: s.id,
      meetingId: d.meeting.id,
      track: s.track,
      speaker: null,
      startMs: s.startMs ?? 0,
      endMs: s.endMs ?? 0,
      text: s.text,
      edited: false,
    })),
    notes: null,
    progress: { done: 0, total: 0 },
    recorded: {
      dir: d.meeting.dir,
      problems: [],
      peaks: d.hasAudio ? d.peaks.map((p) => p ?? 0) : undefined,
    },
  };
}

/** Saved meetings (app) or demo meetings (browser preview). */
export function useMeetings(query: string) {
  return useQuery({
    queryKey: queryKeys.meetings(query),
    queryFn: async (): Promise<MeetingSummary[]> =>
      isTauri()
        ? (await call(commands.meetingsList(query.trim() || null, 200))).map(toSummary)
        : backend.meetingsList(query),
    placeholderData: (prev) => prev,
  });
}

export function useMeeting(id: string | null) {
  return useQuery({
    queryKey: queryKeys.meeting(id ?? ""),
    queryFn: async (): Promise<MeetingDetail> =>
      isTauri()
        ? toDetail(await call(commands.meetingGet(id ?? "")))
        : backend.meetingGet(id ?? ""),
    enabled: id !== null,
  });
}

export function useRenameMeeting() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: async ({ id, title }: { id: string; title: string }) =>
      isTauri() ? call(commands.meetingRename(id, title)) : backend.meetingRename(id, title),
    onSuccess: (_, { id }) => {
      void qc.invalidateQueries({ queryKey: ["meetings"] });
      void qc.invalidateQueries({ queryKey: queryKeys.meeting(id) });
    },
  });
}

export function useDeleteMeeting() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: async (id: string) =>
      isTauri() ? call(commands.meetingDelete(id)) : backend.meetingDelete(id),
    onSuccess: (_, id) => {
      qc.removeQueries({ queryKey: queryKeys.meeting(id) });
      void qc.invalidateQueries({ queryKey: ["meetings"] });
    },
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
