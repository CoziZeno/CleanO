import { useEffect, useState } from "react";
import { api } from "@/ipc/client";
import type { LifetimeStats, OperationRow } from "@/ipc/types";
import { formatBytes, formatCount, plural } from "@/lib/format";
import { IconSparkle } from "./icons";

/**
 * Progress worth feeling.
 *
 * A cleanup tool that only ever shows what is left to do is exhausting - every
 * screen is a list of problems. These two pieces show the opposite: what you
 * already dealt with. The numbers are real, read from the operation journal,
 * and they go *down* if you undo something, because a reward you did not earn
 * is worth nothing.
 */

const MILESTONES: { bytes: number; label: string }[] = [
  { bytes: 100 * 1024 ** 2, label: "100 MB" },
  { bytes: 500 * 1024 ** 2, label: "500 MB" },
  { bytes: 1024 ** 3, label: "1 GB" },
  { bytes: 5 * 1024 ** 3, label: "5 GB" },
  { bytes: 10 * 1024 ** 3, label: "10 GB" },
  { bytes: 25 * 1024 ** 3, label: "25 GB" },
  { bytes: 50 * 1024 ** 3, label: "50 GB" },
  { bytes: 100 * 1024 ** 3, label: "100 GB" },
];

function nextMilestone(bytes: number) {
  return MILESTONES.find((milestone) => milestone.bytes > bytes) ?? null;
}

function previousMilestone(bytes: number) {
  return [...MILESTONES].reverse().find((milestone) => milestone.bytes <= bytes) ?? null;
}

const prefersReducedMotion = () =>
  typeof window !== "undefined" &&
  window.matchMedia?.("(prefers-reduced-motion: reduce)").matches;

/** Counts up to a number with an ease-out, or jumps straight there if the
 *  viewer asked for reduced motion. */
function useCountUp(target: number, durationMs = 900): number {
  const [value, setValue] = useState(target);

  useEffect(() => {
    if (target <= 0 || prefersReducedMotion()) {
      setValue(target);
      return;
    }
    let frame = 0;
    const started = performance.now();
    const tick = (now: number) => {
      const progress = Math.min(1, (now - started) / durationMs);
      const eased = 1 - Math.pow(1 - progress, 3);
      setValue(Math.round(target * eased));
      if (progress < 1) frame = requestAnimationFrame(tick);
    };
    frame = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(frame);
  }, [target, durationMs]);

  return value;
}

/** Shown right after an operation lands. */
export function CleanupCelebration({
  operation,
  onDismiss,
}: {
  operation: OperationRow;
  onDismiss: () => void;
}) {
  const freed = useCountUp(operation.bytes);
  const [lifetime, setLifetime] = useState<LifetimeStats | null>(null);

  useEffect(() => {
    void api.lifetimeStats().then(setLifetime).catch(() => setLifetime(null));
  }, [operation.id]);

  if (operation.done === 0) return null;

  const justPassed =
    lifetime && operation.bytes > 0
      ? MILESTONES.find(
          (milestone) =>
            lifetime.bytes >= milestone.bytes &&
            lifetime.bytes - operation.bytes < milestone.bytes,
        )
      : null;

  const verb = operation.kind === "trash" ? "moved to the Recycle Bin" : "sorted into folders";

  return (
    <div className="reward">
      <div className="reward-burst" aria-hidden="true">
        {Array.from({ length: 7 }).map((_, index) => (
          <span key={index} style={{ ["--i" as string]: index }} />
        ))}
      </div>

      <span className="reward-icon">
        <IconSparkle size={19} />
      </span>

      <div className="reward-body">
        <div className="reward-value">
          {operation.bytes > 0 ? formatBytes(freed) : formatCount(operation.done)}
          {operation.bytes > 0 ? <span className="reward-unit"> freed</span> : null}
        </div>
        <div className="reward-sub">
          {formatCount(operation.done)} {plural(operation.done, "file")} {verb}
          {operation.failed > 0
            ? ` · ${formatCount(operation.failed)} could not be, see History`
            : " · undo any time from History"}
        </div>
        {justPassed ? (
          <div className="reward-milestone">
            That puts you past {justPassed.label} cleared in total.
          </div>
        ) : null}
      </div>

      <button className="btn-ghost reward-dismiss" onClick={onDismiss}>
        Done
      </button>
    </div>
  );
}

/** Running total, shown on the Overview. */
export function LifetimeCard() {
  const [stats, setStats] = useState<LifetimeStats | null>(null);

  useEffect(() => {
    void api.lifetimeStats().then(setStats).catch(() => setStats(null));
  }, []);

  // Nothing cleaned yet means nothing to celebrate; an empty trophy case is
  // worse than no trophy case.
  if (!stats || stats.operations === 0) return null;

  const next = nextMilestone(stats.bytes);
  const previous = previousMilestone(stats.bytes);
  const floor = previous?.bytes ?? 0;
  const progress = next
    ? Math.max(2, Math.min(100, ((stats.bytes - floor) / (next.bytes - floor)) * 100))
    : 100;

  return (
    <div className="lifetime">
      <span className="reward-icon small">
        <IconSparkle size={14} />
      </span>
      <div className="lifetime-body">
        <div className="lifetime-top">
          <span className="lifetime-value">{formatBytes(stats.bytes)} cleared</span>
          <span className="dim" style={{ fontSize: 11.5 }}>
            {formatCount(stats.files)} {plural(stats.files, "file")} across{" "}
            {formatCount(stats.operations)} {plural(stats.operations, "cleanup")}
          </span>
        </div>
        <div className="lifetime-track">
          <div className="lifetime-fill" style={{ width: `${progress}%` }} />
        </div>
        <div className="dim" style={{ fontSize: 11 }}>
          {next
            ? `${formatBytes(next.bytes - stats.bytes)} to go until ${next.label}`
            : "Every milestone cleared."}
        </div>
      </div>
    </div>
  );
}
