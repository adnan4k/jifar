import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect } from "react";
import { commands, type Meeting, type MeetingDetailDto, type TaskDto } from "@/bindings";
import { backend } from "./mock-backend";
import { call } from "./native";
import { isTauri } from "./platform";
import type {
  AudioDevice,
  MeetingDetail,
  MeetingSummary,
  NewTask,
  Task,
  TaskFilter,
  TaskPatch,
} from "./types";

export const queryKeys = {
  meetings: (q: string) => ["meetings", q] as const,
  meeting: (id: string) => ["meeting", id] as const,
  devices: ["devices"] as const,
  /** Everything task-related starts with "tasks", so one key refreshes it all. */
  tasks: ["tasks"] as const,
  taskList: (filter: TaskFilter, q: string) => ["tasks", "list", filter, q] as const,
  meetingTasks: (id: string) => ["tasks", "meeting", id] as const,
  taskCounts: ["tasks", "counts"] as const,
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
    notes: d.notes
      ? {
          summary: d.notes.summary,
          keyPoints: d.notes.keyPoints,
          decisions: d.notes.decisions,
          actionItems: d.notes.actionItems.map((a) => ({
            text: a.text,
            owner: a.owner ?? undefined,
            due: a.due ?? undefined,
            segmentIds: a.segmentIds,
          })),
          keywords: [],
          version: d.notes.version,
          language: d.notes.language,
        }
      : null,
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
      // Tasks show their meeting's title.
      void qc.invalidateQueries({ queryKey: queryKeys.tasks });
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
      // Its tasks were deleted with it.
      void qc.invalidateQueries({ queryKey: queryKeys.tasks });
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

/** Generates (or regenerates) notes; the meeting refetches with them. */
export function useGenerateNotes() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: async ({ id, outputLanguage }: { id: string; outputLanguage: string }) => {
      if (isTauri()) await call(commands.notesGenerate(id, outputLanguage));
      else await backend.notesGenerate(id);
    },
    onSuccess: (_, { id }) => void qc.invalidateQueries({ queryKey: queryKeys.meeting(id) }),
  });
}

// ---- Tasks (FR-AI-08) -------------------------------------------------------

function toTask(t: TaskDto): Task {
  return {
    id: t.id,
    meetingId: t.meetingId ?? null,
    meetingTitle: t.meetingTitle ?? null,
    text: t.text,
    owner: t.owner ?? null,
    due: t.due ?? null,
    done: t.done,
    doneAt: t.doneAt ?? null,
    sourceKind: t.sourceKind,
    sourceVersion: t.sourceVersion ?? null,
    segmentIds: t.segmentIds,
    sourceStartMs: t.sourceStartMs ?? null,
    matchKey: t.matchKey,
    createdAt: t.createdAt ?? 0,
  };
}

async function listTasks(
  filter: TaskFilter,
  meetingId: string | null,
  query: string,
): Promise<Task[]> {
  return isTauri()
    ? (await call(commands.tasksList(filter, meetingId, query.trim() || null))).map(toTask)
    : backend.tasksList(filter, meetingId, query);
}

/** Tasks across meetings (the Tasks view). */
export function useTasks(filter: TaskFilter, query: string) {
  return useQuery({
    queryKey: queryKeys.taskList(filter, query),
    queryFn: () => listTasks(filter, null, query),
    placeholderData: (prev) => prev,
  });
}

/** One meeting's tasks: "In tasks" marks in its notes, the delete count. */
export function useMeetingTasks(meetingId: string | null, enabled = true) {
  return useQuery({
    queryKey: queryKeys.meetingTasks(meetingId ?? ""),
    queryFn: () => listTasks("all", meetingId, ""),
    enabled: enabled && meetingId !== null,
  });
}

export function useTaskCounts() {
  return useQuery({
    queryKey: queryKeys.taskCounts,
    queryFn: async () => (isTauri() ? call(commands.tasksCounts()) : backend.tasksCounts()),
  });
}

function useTaskMutation<A, R>(fn: (args: A) => Promise<R>) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: fn,
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: queryKeys.tasks });
      // Meeting search matches task words.
      void qc.invalidateQueries({ queryKey: ["meetings"] });
    },
  });
}

export function useAddTasks() {
  return useTaskMutation(
    async (items: NewTask[]): Promise<Task[]> =>
      isTauri()
        ? (
            await call(
              commands.tasksAdd(
                items.map((i) => ({
                  meetingId: i.meetingId,
                  text: i.text,
                  owner: i.owner ?? null,
                  due: i.due ?? null,
                  sourceKind: i.sourceKind,
                  sourceVersion: i.sourceVersion ?? null,
                  segmentIds: i.segmentIds,
                })),
              ),
            )
          ).map(toTask)
        : backend.tasksAdd(items),
  );
}

export function useUpdateTask() {
  return useTaskMutation(
    async ({ id, patch }: { id: string; patch: TaskPatch }): Promise<Task> =>
      isTauri()
        ? toTask(
            await call(
              commands.taskUpdate(id, {
                text: patch.text ?? null,
                owner: patch.owner ?? null,
                due: patch.due ?? null,
                done: patch.done ?? null,
              }),
            ),
          )
        : backend.taskUpdate(id, patch),
  );
}

export function useDeleteTask() {
  return useTaskMutation(
    async (id: string): Promise<null> =>
      isTauri() ? call(commands.taskDelete(id)) : backend.taskDelete(id),
  );
}

export function useRetryQueue() {
  return useMutation({ mutationFn: (id: string) => backend.queueRetry(id) });
}
