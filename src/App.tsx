import { NotesPanel } from "@/components/notes/NotesPanel";
import { Sidebar } from "@/components/sidebar/Sidebar";
import { TranscriptPane } from "@/components/transcript/TranscriptPane";
import { useBackendEvents } from "@/lib/queries";

// Three-column layout: docs/DESIGN.md §2.
export function App() {
  useBackendEvents();
  return (
    <div className="flex h-full">
      <Sidebar />
      <TranscriptPane />
      <NotesPanel />
    </div>
  );
}
