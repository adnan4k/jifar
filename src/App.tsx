import { useEffect } from "react";
import { NotesPanel } from "@/components/notes/NotesPanel";
import { SettingsDialog } from "@/components/settings/SettingsDialog";
import { Sidebar } from "@/components/sidebar/Sidebar";
import { TranscriptPane } from "@/components/transcript/TranscriptPane";
import { useBackendEvents } from "@/lib/queries";
import { connectRecording } from "@/stores/recording";

// Three-column layout: docs/DESIGN.md §2.
export function App() {
  useBackendEvents();
  useEffect(() => connectRecording(), []);
  return (
    <div className="flex h-full">
      <Sidebar />
      <TranscriptPane />
      <NotesPanel />
      <SettingsDialog />
    </div>
  );
}
