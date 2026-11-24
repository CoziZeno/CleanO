import { useCallback, useEffect, useState } from "react";
import type { Route } from "@/app/nav";
import {
  IconDuplicates,
  IconQueue,
  IconSparkle,
  IconStorage,
} from "@/components/icons";
import { api, toIpcError } from "@/ipc/client";
import { QuickClean } from "./QuickClean";
import type { InstallerMatchRow, QueryRequest, SwipeProgress } from "@/ipc/types";
import { formatBytes, formatCount, plural } from "@/lib/format";
import { useDupStore } from "@/state/dupStore";
import { useScanStore } from "@/state/scanStore";

type Jump = (route: Route, preset?: Partial<QueryRequest>) => void;

interface Task {
  id: string;
  title: string;
  /** Why this is being suggested. Never a scare tactic. */
  reason: string;
  files: number;
  /** Only set when it is genuinely measurable. */
  bytes: number | null;
  /** Null when we cannot honestly estimate it. */
  minutes: number | null;
  actionLabel: string;
  run: () => void;
  icon: JSX.Element;
}

/**
 * A session is a short, ordered list of things worth doing right now.
 *
 * Every number here comes from a real measurement. Where something is not
 * measurable - how long a review will take, how much space unverified
 * duplicates would free - it is left out rather than invented, which is why
 * some tasks show no time and no size.
 */
export function SessionsView({ onJump }: { onJump: Jump }) {
  const { summary, generation } = useScanStore();
  const dupReport = useDupStore((state) => state.report);
  const startDedupe = useDupStore((state) => state.start);

  const [installers, setInstallers] = useState<InstallerMatchRow[]>([]);
  const [review, setReview] = useState<SwipeProgress | null>(null);
  const [done, setDone] = useState<Set<string>>(new Set());

  const load = useCallback(async () => {
    try {
      setInstallers(await api.matchInstallers());
    } catch {
      setInstallers([]);
    }
    try {
      setReview((await api.swipeQueue(false)).progress);
    } catch (err) {
      if (toIpcError(err).code === "noScan") setReview(null);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load, generation]);

  if (!summary) {
    return (
      <div className="view">
        <QuickClean />
        <div className="empty">
          <div className="empty-title">Nothing scanned yet</div>
          <p className="empty-body">
            Scan a folder and Cleanup will put together a short list of things worth doing.
          </p>
        </div>
      </div>
    );
  }

  const tasks: Task[] = [];

  // 1. Confirmed duplicates — the only task with a guaranteed saving.
  if (dupReport && dupReport.stats.groups > 0) {
    tasks.push({
      id: "duplicates",
      title: `${formatCount(dupReport.stats.groups)} confirmed duplicate ${plural(dupReport.stats.groups, "group")}`,
      reason:
        "Contents compared byte for byte, so these are genuinely identical. Keeping one copy of each is the only task here with a guaranteed saving.",
      files: dupReport.stats.duplicateFiles,
      bytes: dupReport.stats.reclaimable,
      minutes: null,
      actionLabel: "Review duplicates",
      run: () => onJump("duplicates"),
      icon: <IconDuplicates size={15} />,
    });
  } else if (summary.duplicateCandidates.groups > 0) {
    tasks.push({
      id: "dupe-check",
      title: `${formatCount(summary.duplicateCandidates.files)} files share a size`,
      reason:
        "Same size is a hint, not proof. Comparing contents is what turns this into a real number — nothing is counted as recoverable until then.",
      files: summary.duplicateCandidates.files,
      bytes: null,
      minutes: null,
      actionLabel: "Compare contents",
      run: () => {
        onJump("duplicates");
        void startDedupe();
      },
      icon: <IconDuplicates size={15} />,
    });
  }

  // 2. Unreviewed media — the Swipe Clean queue.
  if (review && review.remaining > 0) {
    tasks.push({
      id: "review",
      title: `${formatCount(review.remaining)} ${plural(review.remaining, "file")} to look through`,
      reason:
        "Images, videos, archives and large files, biggest first. One at a time, and keeping a file moves nothing.",
      files: review.remaining,
      bytes: null,
      minutes: null,
      actionLabel: "Start reviewing",
      run: () => onJump("swipe"),
      icon: <IconSparkle size={15} />,
    });
  }

  // 3. Installers you already have — a fact, not a guess about age.
  const redundant = installers.filter(
    (row) => row.matched.verdict === "older" || row.matched.verdict === "same",
  );
  if (redundant.length > 0) {
    tasks.push({
      id: "installers",
      title: `${formatCount(redundant.length)} ${plural(redundant.length, "installer")} you already installed`,
      reason:
        "Each one matches software already on this machine, at the same version or older. That is a reason to remove it, rather than a guess based on age.",
      files: redundant.length,
      bytes: redundant.reduce((sum, row) => sum + row.file.size, 0),
      minutes: null,
      actionLabel: "Review installers",
      run: () => onJump("apps"),
      icon: <IconQueue size={15} />,
    });
  }

  // 4. Loose files worth filing.
  const loose = summary.byCategory.find((c) => c.category === "images");
  if (loose && loose.count > 0) {
    tasks.push({
      id: "organize",
      title: "Sort loose files into folders",
      reason:
        "Preview shows exactly where each file would go, including any renames a name clash would force. Nothing moves until you apply it.",
      files: summary.totalFiles,
      bytes: null,
      minutes: null,
      actionLabel: "Preview sorting",
      run: () => onJump("downloads"),
      icon: <IconStorage size={15} />,
    });
  }

  // 5. Review Later backlog.
  if (review && review.later > 0) {
    tasks.push({
      id: "later",
      title: `${formatCount(review.later)} skipped ${plural(review.later, "file")} waiting`,
      reason: "Files you chose to decide on another time.",
      files: review.later,
      bytes: null,
      minutes: null,
      actionLabel: "Look again",
      run: () => onJump("swipe"),
      icon: <IconSparkle size={15} />,
    });
  }

  const remaining = tasks.filter((task) => !done.has(task.id));
  const confirmedBytes = dupReport?.stats.reclaimable ?? 0;

  return (
    <div className="view">
      <QuickClean />

      <div className="session-head">
        <h2 className="session-title">
          {remaining.length === 0
            ? "Nothing needs your attention"
            : `${remaining.length} ${plural(remaining.length, "thing")} worth doing in ${summary.rootDisplay}`}
        </h2>
        <p className="session-sub">
          {remaining.length === 0
            ? "Everything Cleanup can measure is already in order. Scan again after you have downloaded more."
            : "Work through these in any order. Nothing here deletes anything on its own."}
        </p>
      </div>

      <div className="stat-grid">
        <div className="stat">
          <div className="stat-label">Indexed</div>
          <div className="stat-value">{formatBytes(summary.totalBytes)}</div>
          <div className="stat-foot">{formatCount(summary.totalFiles)} files</div>
        </div>
        <div className="stat">
          <div className="stat-label">Confirmed recoverable</div>
          <div className="stat-value">{formatBytes(confirmedBytes)}</div>
          <div className="stat-foot">
            {confirmedBytes > 0 ? "verified by content" : "run the duplicate check"}
          </div>
        </div>
        <div className="stat">
          <div className="stat-label">Possible, unverified</div>
          <div className="stat-value">
            {formatBytes(summary.duplicateCandidates.reclaimableBytesUpperBound)}
          </div>
          <div className="stat-foot">an upper bound, from size alone</div>
        </div>
      </div>

      {remaining.length > 0 ? (
        <div className="signal-list">
          {remaining.map((task) => (
            <div className="session-task" key={task.id}>
              <span className="signal-icon">{task.icon}</span>
              <div className="signal-body">
                <div className="session-task-title">{task.title}</div>
                <div className="signal-why">{task.reason}</div>
                <div className="session-task-facts">
                  <span>{formatCount(task.files)} {plural(task.files, "file")}</span>
                  {task.bytes !== null ? (
                    <>
                      <span className="dim">·</span>
                      <span>{formatBytes(task.bytes)}</span>
                    </>
                  ) : null}
                  {task.minutes !== null ? (
                    <>
                      <span className="dim">·</span>
                      <span>~{task.minutes} min</span>
                    </>
                  ) : null}
                </div>
              </div>
              <div className="session-task-actions">
                <button className="btn btn-primary" onClick={task.run}>
                  {task.actionLabel}
                </button>
                <button
                  className="btn btn-ghost"
                  onClick={() => setDone((current) => new Set(current).add(task.id))}
                >
                  Skip
                </button>
              </div>
            </div>
          ))}
        </div>
      ) : null}

      {done.size > 0 ? (
        <button className="btn btn-ghost" onClick={() => setDone(new Set())}>
          Show {done.size} skipped {plural(done.size, "task")}
        </button>
      ) : null}
    </div>
  );
}
