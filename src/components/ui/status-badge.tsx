import { AlertTriangle, Loader2 } from "lucide-react";
import type { MeetingStatus } from "@/lib/types";
import { cn } from "@/lib/utils";

export function StatusBadge({ status, className }: { status: MeetingStatus; className?: string }) {
  if (status === "ready") return null;
  const map = {
    recording: {
      label: "Recording",
      cls: "text-danger",
      icon: <span className="size-1.5 animate-pulse rounded-full bg-danger" />,
    },
    processing: {
      label: "Transcribing",
      cls: "text-accent",
      icon: <Loader2 className="size-3 animate-spin" />,
    },
    needs_attention: {
      label: "Needs attention",
      cls: "text-warn",
      icon: <AlertTriangle className="size-3" />,
    },
  } as const;
  const { label, cls, icon } = map[status];
  return (
    <span className={cn("inline-flex items-center gap-1 text-[11px] font-medium", cls, className)}>
      {icon}
      {label}
    </span>
  );
}
