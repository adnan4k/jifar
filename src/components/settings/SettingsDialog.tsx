import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  CheckCircle2,
  KeyRound,
  Loader2,
  Mic,
  ShieldCheck,
  Trash2,
  Volume2,
  X,
  XCircle,
} from "lucide-react";
import { type FormEvent, type ReactNode, useEffect, useRef, useState } from "react";
import { commands, type ProviderKeyTest_Serialize } from "@/bindings";
import { Button } from "@/components/ui/button";
import { call, describeError } from "@/lib/native";
import { isTauri } from "@/lib/platform";
import { useDevices } from "@/lib/queries";
import { currentSettings, saveSettings, useRecording } from "@/stores/recording";
import { useUi } from "@/stores/ui";

// Settings (FR-PRV-03, FR-PRV-04, FR-REC-07): provider key, devices, and a
// plain statement of where data goes.

export function SettingsDialog() {
  const { settingsOpen, setSettingsOpen } = useUi();
  const close = () => setSettingsOpen(false);

  useEffect(() => {
    if (!settingsOpen) return;
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && setSettingsOpen(false);
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [settingsOpen, setSettingsOpen]);

  if (!settingsOpen) return null;
  return (
    <div className="fixed inset-0 z-50 grid place-items-center bg-black/60 p-6">
      <button
        type="button"
        aria-label="Close settings"
        className="absolute inset-0"
        onClick={close}
      />
      <div
        role="dialog"
        aria-modal="true"
        aria-labelledby="settings-title"
        className="relative flex max-h-full w-full max-w-xl flex-col overflow-hidden rounded-2xl border bg-panel shadow-2xl"
      >
        <header className="flex h-14 shrink-0 items-center justify-between border-b px-6">
          <h2 id="settings-title" className="font-medium">
            Settings
          </h2>
          <Button variant="ghost" size="icon" onClick={close} aria-label="Close">
            <X />
          </Button>
        </header>
        <div className="min-h-0 space-y-8 overflow-y-auto p-6">
          {isTauri() ? (
            <>
              <ApiKeySection />
              <DevicesSection />
              <DataFlowSection />
            </>
          ) : (
            <p className="text-sm text-muted">
              Settings are available in the desktop app. This browser preview uses mock data.
            </p>
          )}
        </div>
      </div>
    </div>
  );
}

function Section({
  icon,
  title,
  children,
}: {
  icon: ReactNode;
  title: string;
  children: ReactNode;
}) {
  return (
    <section className="space-y-3">
      <h3 className="flex items-center gap-2 text-sm font-medium">
        {icon}
        {title}
      </h3>
      {children}
    </section>
  );
}

const keyStatusKey = ["providerKey", "addis_scribe"] as const;

function ApiKeySection() {
  const qc = useQueryClient();
  const [draft, setDraft] = useState("");
  const [test, setTest] = useState<ProviderKeyTest_Serialize | null>(null);
  const [error, setError] = useState<string | null>(null);
  const input = useRef<HTMLInputElement>(null);

  const status = useQuery({
    queryKey: keyStatusKey,
    queryFn: () => call(commands.providerKeyStatus("addis_scribe")),
  });

  const runTest = useMutation({
    mutationFn: () => call(commands.providerKeyTest("addis_scribe")),
    onSuccess: setTest,
    onError: (e) => setError(describeError(e)),
  });

  const save = useMutation({
    mutationFn: (key: string) => call(commands.providerKeySet("addis_scribe", key)),
    onSuccess: (s) => {
      qc.setQueryData(keyStatusKey, s);
      setDraft("");
      setError(null);
      setTest(null);
      // Check right away, so a typo shows up now rather than after a meeting.
      runTest.mutate();
    },
    onError: (e) => setError(describeError(e)),
  });

  const remove = useMutation({
    mutationFn: () => call(commands.providerKeyDelete("addis_scribe")),
    onSuccess: (s) => {
      qc.setQueryData(keyStatusKey, s);
      setTest(null);
    },
    onError: (e) => setError(describeError(e)),
  });

  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (draft.trim()) save.mutate(draft);
  };

  const s = status.data;
  return (
    <Section icon={<KeyRound className="size-4 text-accent" />} title="Transcription: Addis Scribe">
      {s?.keyringError ? (
        <p className="rounded-lg bg-warn/10 px-3 py-2 text-xs text-warn">
          The system keyring isn't available, so the key can't be stored safely: {s.keyringError}
        </p>
      ) : (
        <p className="text-sm text-muted">
          {s?.saved ? (
            <>
              A key is saved in the system keyring{" "}
              <span className="font-mono text-fg">(…{s.hint})</span>.
            </>
          ) : (
            "No API key yet. Get one at addisassistant.com/apikeys (it starts with sk_)."
          )}
        </p>
      )}

      <form onSubmit={submit} className="flex gap-2">
        <input
          ref={input}
          type="password"
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          placeholder={s?.saved ? "Paste a new key to replace it" : "sk_…"}
          autoComplete="off"
          spellCheck={false}
          aria-label="Addis Scribe API key"
          className="h-9 min-w-0 flex-1 rounded-lg border bg-bg px-3 font-mono text-sm outline-none placeholder:font-sans placeholder:text-muted focus-visible:ring-2 focus-visible:ring-accent/60"
        />
        <Button type="submit" disabled={!draft.trim() || save.isPending}>
          {save.isPending && <Loader2 className="animate-spin" />}
          Save
        </Button>
      </form>

      {s?.saved && (
        <div className="flex items-center gap-2">
          <Button
            variant="outline"
            size="sm"
            onClick={() => runTest.mutate()}
            disabled={runTest.isPending}
          >
            {runTest.isPending && <Loader2 className="animate-spin" />}
            Test key
          </Button>
          <Button
            variant="ghost"
            size="sm"
            onClick={() => remove.mutate()}
            disabled={remove.isPending}
          >
            <Trash2 />
            Remove
          </Button>
        </div>
      )}

      {test && <TestResult test={test} />}
      {error && <p className="text-xs text-warn">{error}</p>}
    </Section>
  );
}

function TestResult({ test }: { test: ProviderKeyTest_Serialize }) {
  const ok = test.status === "ok";
  const text = (() => {
    switch (test.status) {
      case "ok":
        return test.balance != null
          ? `Key works. Balance: ${test.balance.toLocaleString()} ${test.currency ?? ""}`.trim()
          : "Key works.";
      case "no_key":
        return "No key saved.";
      case "invalid_key":
        return "Addis AI rejected this key. Check it, or create a new one.";
      case "no_credits":
        return "The key works, but the balance is too low. Top up in the Addis AI dashboard.";
      case "unreachable":
        return "Couldn't reach Addis AI (offline or busy). The key may be fine; try again later.";
      default:
        return `Test failed: ${test.message ?? "unknown error"}`;
    }
  })();
  return (
    <p className={`flex items-start gap-2 text-sm ${ok ? "text-accent" : "text-warn"}`}>
      {ok ? (
        <CheckCircle2 className="mt-0.5 size-4 shrink-0" />
      ) : (
        <XCircle className="mt-0.5 size-4 shrink-0" />
      )}
      {text}
    </p>
  );
}

function DevicesSection() {
  const { data, isError, error } = useDevices();
  const recording = useRecording((s) => s.phase !== "idle");
  const micDevice = useRecording((s) => s.micDevice);
  const [sink, setSink] = useState(currentSettings().sinkDevice ?? "");
  const [saveError, setSaveError] = useState<string | null>(null);

  const change = (patch: { micDevice?: string | null; sinkDevice?: string | null }) => {
    setSaveError(null);
    saveSettings(patch).catch((e) => setSaveError(describeError(e)));
  };

  const select =
    "h-9 w-full rounded-lg border bg-bg px-2 text-sm outline-none focus-visible:ring-2 focus-visible:ring-accent/60 disabled:opacity-60";
  return (
    <Section icon={<Mic className="size-4 text-accent" />} title="Audio devices">
      {isError && (
        <p className="text-xs text-warn">Couldn't list devices: {describeError(error)}</p>
      )}
      <label className="block space-y-1.5">
        <span className="text-xs text-muted">Microphone (Me)</span>
        <select
          className={select}
          value={micDevice}
          disabled={recording}
          onChange={(e) => change({ micDevice: e.target.value || null })}
        >
          <option value="">System default microphone</option>
          {data?.inputs.map((d) => (
            <option key={d.id} value={d.id}>
              {d.label}
              {d.isDefault ? " (default)" : ""}
            </option>
          ))}
        </select>
      </label>
      <label className="block space-y-1.5">
        <span className="flex items-center gap-1.5 text-xs text-muted">
          <Volume2 className="size-3.5" />
          Other participants: what this output plays is recorded
        </span>
        <select
          className={select}
          value={sink}
          disabled={recording}
          onChange={(e) => {
            setSink(e.target.value);
            change({ sinkDevice: e.target.value || null });
          }}
        >
          <option value="">System default output</option>
          {data?.outputs.map((d) => (
            <option key={d.id} value={d.id}>
              {d.label}
              {d.isDefault ? " (default)" : ""}
            </option>
          ))}
        </select>
      </label>
      {recording && <p className="text-xs text-muted">Devices can't change while recording.</p>}
      {saveError && <p className="text-xs text-warn">{saveError}</p>}
    </Section>
  );
}

function DataFlowSection() {
  return (
    <Section icon={<ShieldCheck className="size-4 text-accent" />} title="Where your data goes">
      <ul className="space-y-2 text-sm text-muted">
        <li>
          <span className="text-fg">Recordings stay on this computer.</span> Audio is saved in your
          home folder and is never uploaded as a whole.
        </li>
        <li>
          <span className="text-fg">After a meeting, speech goes to Addis AI</span> (Addis Scribe)
          to be transcribed, in short pieces. Silence is never sent. Nothing is sent while you
          record.
        </li>
        <li>
          <span className="text-fg">Notes:</span> when you generate notes, the transcript text is
          sent to the notes provider (not connected yet).
        </li>
        <li>
          <span className="text-fg">Your API key</span> is kept in the system keyring, not in a
          file. Jifar has no account and sends no usage data.
        </li>
      </ul>
    </Section>
  );
}
