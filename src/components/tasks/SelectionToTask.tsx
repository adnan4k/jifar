import { Check, ListPlus, Loader2 } from "lucide-react";
import { type ReactNode, useCallback, useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { describeError } from "@/lib/native";
import { useAddTasks } from "@/lib/queries";
import { matchKey } from "@/lib/tasks";
import type { TaskSourceKind } from "@/lib/types";

/** Longer selections are probably accidental (select-all, a whole page). */
const MAX_CHARS = 500;

interface Pick {
  text: string;
  kind: TaskSourceKind;
  segmentIds: string[];
  /** Viewport position of the selection's top centre (or bottom, if near the top). */
  x: number;
  y: number;
  below: boolean;
}

/**
 * Select any text inside to add it as a task (FR-AI-08). The source kind
 * comes from the nearest `data-task-kind` ancestor (else `kind`), and the
 * jump-to-source segments from the nearest `data-segment-ids` (space
 * separated).
 */
export function SelectionToTask({
  meetingId,
  sourceVersion,
  kind,
  className,
  children,
}: {
  meetingId: string;
  sourceVersion: number | null;
  kind: TaskSourceKind;
  className?: string;
  children: ReactNode;
}) {
  const root = useRef<HTMLDivElement>(null);
  const add = useAddTasks();
  const [pick, setPick] = useState<Pick | null>(null);
  const [notice, setNotice] = useState<{ text: string; x: number; y: number } | null>(null);

  const read = useCallback(() => {
    const sel = window.getSelection();
    const el = root.current;
    if (!sel || sel.isCollapsed || sel.rangeCount === 0 || !el) return setPick(null);
    const range = sel.getRangeAt(0);
    if (!el.contains(range.commonAncestorContainer)) return setPick(null);
    const text = matchKey(sel.toString());
    if (!text) return setPick(null);
    const node = range.startContainer;
    const start = node instanceof Element ? node : node.parentElement;
    const source = start?.closest<HTMLElement>("[data-task-kind]");
    const segments = start?.closest<HTMLElement>("[data-segment-ids]");
    const rect = range.getBoundingClientRect();
    const below = rect.top < 48;
    setPick({
      text,
      kind: (source?.dataset.taskKind as TaskSourceKind | undefined) ?? kind,
      segmentIds: segments?.dataset.segmentIds?.split(" ").filter(Boolean) ?? [],
      x: rect.left + rect.width / 2,
      y: below ? rect.bottom : rect.top,
      below,
    });
  }, [kind]);

  useEffect(() => {
    // Selection cleared (click elsewhere, Escape): hide. Scrolling: follow it.
    const onChange = () => {
      const sel = window.getSelection();
      if (!sel || sel.isCollapsed) setPick(null);
    };
    const onScroll = () => {
      if (window.getSelection()?.isCollapsed === false) read();
    };
    document.addEventListener("selectionchange", onChange);
    window.addEventListener("scroll", onScroll, true);
    return () => {
      document.removeEventListener("selectionchange", onChange);
      window.removeEventListener("scroll", onScroll, true);
    };
  }, [read]);

  useEffect(() => {
    if (!notice) return;
    const t = setTimeout(() => setNotice(null), 2_000);
    return () => clearTimeout(t);
  }, [notice]);

  // Let the browser finish the selection before reading it.
  const later = () => setTimeout(read, 0);
  const tooLong = (pick?.text.length ?? 0) > MAX_CHARS;

  const submit = () => {
    if (!pick || tooLong) return;
    const at = { x: pick.x, y: pick.y };
    add.mutate(
      [
        {
          meetingId,
          text: pick.text,
          sourceKind: pick.kind,
          sourceVersion,
          segmentIds: pick.segmentIds,
        },
      ],
      {
        onSuccess: () => {
          window.getSelection()?.removeAllRanges();
          setPick(null);
          setNotice({ text: "Added to tasks", ...at });
        },
        onError: (e) => setNotice({ text: `Couldn't add: ${describeError(e)}`, ...at }),
      },
    );
  };

  return (
    // biome-ignore lint/a11y/noStaticElementInteractions: only listens for text selection; the add button itself is keyboard reachable
    <div ref={root} onMouseUp={later} onKeyUp={later} className={className}>
      {children}
      {pick &&
        createPortal(
          <button
            type="button"
            // Keep the selection when the button is pressed.
            onMouseDown={(e) => e.preventDefault()}
            onClick={submit}
            disabled={add.isPending || tooLong}
            title={tooLong ? `Select at most ${MAX_CHARS} characters` : pick.text}
            style={{
              left: pick.x,
              top: pick.y,
              transform: `translate(-50%, ${pick.below ? "8px" : "calc(-100% - 8px)"})`,
            }}
            className="fixed z-50 flex items-center gap-1.5 rounded-lg border bg-panel px-2.5 py-1.5 text-xs font-medium text-fg shadow-lg transition-colors hover:bg-card-hover hover:text-accent disabled:text-muted [&_svg]:size-3.5"
          >
            {add.isPending ? <Loader2 className="animate-spin" /> : <ListPlus />}
            {tooLong ? "Selection too long" : "Add to tasks"}
          </button>,
          document.body,
        )}
      {notice &&
        !pick &&
        createPortal(
          <div
            role="status"
            style={{
              left: notice.x,
              top: notice.y,
              transform: "translate(-50%, calc(-100% - 8px))",
            }}
            className="pointer-events-none fixed z-50 flex items-center gap-1.5 rounded-lg border bg-panel px-2.5 py-1.5 text-xs text-fg shadow-lg [&_svg]:size-3.5"
          >
            <Check className="text-accent" />
            {notice.text}
          </div>,
          document.body,
        )}
    </div>
  );
}
