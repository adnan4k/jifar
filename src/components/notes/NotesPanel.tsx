import { CornerDownRight, Loader2, RefreshCw, Settings, Sparkles } from "lucide-react";
import { type ReactNode, useState } from "react";
import { Button } from "@/components/ui/button";
import { CoreError, describeError } from "@/lib/native";
import { isTauri } from "@/lib/platform";
import { useGenerateNotes, useMeeting } from "@/lib/queries";
import type { MeetingDetail, MeetingNotes } from "@/lib/types";
import { useUi } from "@/stores/ui";

export function NotesPanel() {
  const selectedId = useUi((s) => s.selectedMeetingId);
  const { data: meeting } = useMeeting(selectedId);

  return (
    <aside className="flex w-[22rem] shrink-0 flex-col border-l bg-panel">
      <div className="flex h-14 shrink-0 items-center gap-2 border-b px-5">
        <Sparkles className="size-5 text-accent" />
        <h2 className="font-medium">AI Notes</h2>
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto p-5">
        <NotesBody meeting={meeting} />
      </div>
    </aside>
  );
}

const OUTPUT_LANGUAGES = [
  { code: "om", label: "Afaan Oromo" },
  { code: "en", label: "English" },
] as const;

function NotesBody({ meeting }: { meeting: MeetingDetail | undefined }) {
  const generate = useGenerateNotes();
  const [language, setLanguage] = useState<string | null>(null);

  if (!meeting)
    return <Hint>Select a meeting to see its summary, decisions and action items.</Hint>;
  if (meeting.status === "recording")
    return <Hint>Notes are generated from the transcript after the meeting.</Hint>;
  if (meeting.segments.length === 0)
    return (
      <div className="space-y-3">
        <Hint>
          {meeting.status === "processing"
            ? "Waiting for the transcript…"
            : "Notes are made from the transcript once there is one."}
        </Hint>
        {meeting.status === "processing" &&
          [80, 95, 70].map((w) => (
            <div key={w} className="h-3 animate-pulse rounded bg-card" style={{ width: `${w}%` }} />
          ))}
      </div>
    );

  const busy = generate.isPending && generate.variables?.id === meeting.id;
  // Default: the language the notes were last written in, else the meeting's.
  const output = language ?? meeting.notes?.language ?? meeting.language;
  const run = () => generate.mutate({ id: meeting.id, outputLanguage: output });
  const error = generate.isError && generate.variables?.id === meeting.id ? generate.error : null;

  const controls = (
    <div className="space-y-2">
      {isTauri() && (
        <label className="flex items-center justify-between gap-2 text-xs text-muted">
          Write notes in
          <select
            value={output}
            onChange={(e) => setLanguage(e.target.value)}
            disabled={busy}
            className="h-8 rounded-md border bg-bg px-2 text-xs text-fg outline-none focus-visible:ring-2 focus-visible:ring-accent/60"
          >
            {OUTPUT_LANGUAGES.map((l) => (
              <option key={l.code} value={l.code}>
                {l.label}
              </option>
            ))}
          </select>
        </label>
      )}
      <Button
        className="w-full"
        variant={meeting.notes ? "outline" : "default"}
        size={meeting.notes ? "sm" : "default"}
        disabled={busy}
        onClick={run}
      >
        {busy ? <Loader2 className="animate-spin" /> : meeting.notes ? <RefreshCw /> : <Sparkles />}
        {busy ? "Generating…" : meeting.notes ? "Regenerate" : "Generate notes"}
      </Button>
      {error && <NotesError error={error} />}
    </div>
  );

  if (!meeting.notes)
    return (
      <div className="space-y-4">
        <Hint>
          Create a summary with key points, decisions and action items from this transcript. The
          transcript text is sent to Addis AI.
        </Hint>
        {controls}
      </div>
    );

  return (
    <div className="space-y-6">
      <Notes notes={meeting.notes} meeting={meeting} />
      {controls}
    </div>
  );
}

function NotesError({ error }: { error: Error }) {
  const code = error instanceof CoreError ? error.code : null;
  const openSettings = () => useUi.getState().setSettingsOpen(true);
  const text = (() => {
    switch (code) {
      case "provider_auth":
        return "Add a working Addis AI API key in Settings to generate notes.";
      case "provider_credits":
        return "Your Addis AI balance is too low. Top up, then try again.";
      case "provider_unavailable":
        return "Couldn't reach Addis AI (offline or busy). Try again in a moment.";
      case "provider_failed":
        return `The notes model couldn't produce notes: ${error.message}`;
      default:
        return describeError(error);
    }
  })();
  return (
    <div className="space-y-2 rounded-lg bg-warn/10 px-3 py-2 text-xs text-warn">
      <p>{text}</p>
      {(code === "provider_auth" || code === "provider_credits") && (
        <Button size="sm" variant="outline" onClick={openSettings}>
          <Settings />
          Open Settings
        </Button>
      )}
    </div>
  );
}

function Notes({ notes, meeting }: { notes: MeetingNotes; meeting: MeetingDetail }) {
  const seek = useUi((s) => s.seek);
  const jump = (segmentIds: string[]) => {
    const seg = meeting.segments.find((s) => s.id === segmentIds[0]);
    if (seg) seek(seg.startMs);
  };

  return (
    <>
      <Section title="Summary">
        <p className="text-sm leading-relaxed text-fg/90">{notes.summary}</p>
      </Section>

      <Section title="Key points">
        <ul className="list-disc space-y-1.5 pl-5 text-sm text-fg/90 marker:text-accent">
          {notes.keyPoints.map((p) => (
            <li key={p}>{p}</li>
          ))}
        </ul>
      </Section>

      <Section title="Decisions">
        <ul className="space-y-2">
          {notes.decisions.map((d) => (
            <li key={d.text}>
              <SourceLink onClick={() => jump(d.segmentIds)}>{d.text}</SourceLink>
            </li>
          ))}
        </ul>
      </Section>

      <Section title="Action items">
        <ul className="space-y-2">
          {notes.actionItems.map((a) => (
            <li key={a.text}>
              <SourceLink onClick={() => jump(a.segmentIds)}>
                <span>{a.text}</span>
                {(a.owner || a.due) && (
                  <span className="mt-0.5 block text-xs text-muted">
                    {[a.owner, a.due].filter(Boolean).join(" · ")}
                  </span>
                )}
              </SourceLink>
            </li>
          ))}
        </ul>
      </Section>

      {notes.keywords.length > 0 && (
        <Section title="Keywords">
          <div className="flex flex-wrap gap-2">
            {notes.keywords.map((k) => (
              <span key={k} className="rounded-md border bg-card px-2 py-1 text-xs text-fg/90">
                {k}
              </span>
            ))}
          </div>
        </Section>
      )}
    </>
  );
}

function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="space-y-2.5 border-b pb-5 last-of-type:border-0">
      <h3 className="text-sm font-semibold">{title}</h3>
      {children}
    </section>
  );
}

function SourceLink({ children, onClick }: { children: ReactNode; onClick: () => void }) {
  return (
    <button
      type="button"
      onClick={onClick}
      title="Jump to transcript"
      className="group flex w-full gap-2 rounded-md p-1 -m-1 text-left text-sm text-fg/90 hover:bg-card-hover"
    >
      <CornerDownRight className="mt-0.5 size-3.5 shrink-0 text-muted group-hover:text-accent" />
      <span>{children}</span>
    </button>
  );
}

function Hint({ children }: { children: ReactNode }) {
  return <p className="text-sm leading-relaxed text-muted">{children}</p>;
}
