import { useEffect } from "react";
import { NotesPanel } from "@/components/notes/NotesPanel";
import { SettingsDialog } from "@/components/settings/SettingsDialog";
import { Sidebar } from "@/components/sidebar/Sidebar";
import { TasksPane } from "@/components/tasks/TasksPane";
import { TranscriptPane } from "@/components/transcript/TranscriptPane";
import { useBackendEvents } from "@/lib/queries";
import { connectRecording } from "@/stores/recording";
import { useUi } from "@/stores/ui";

// Three-column layout: docs/DESIGN.md §2.
export function App() {
  useBackendEvents();
  useEffect(() => connectRecording(), []);
  const view = useUi((s) => s.view);
  return (
    <div className="flex h-full">
      <Sidebar />
      {view === "tasks" ? <TasksPane /> : <TranscriptPane />}
      <NotesPanel />
      <SettingsDialog />
    </div>
  );
}
