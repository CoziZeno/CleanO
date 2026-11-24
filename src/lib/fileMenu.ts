import type { MenuItem } from "@/components/ContextMenu";
import type { FileRow } from "@/ipc/types";
import { isSafeToTouch, untouchableReason } from "@/ipc/types";
import { useFileOpsStore } from "@/state/fileOpsStore";
import { useQueueStore } from "@/state/queueStore";

const isMac = navigator.platform.toLowerCase().includes("mac");
const mod = isMac ? "⌘" : "Ctrl";

/**
 * The context menu for a file row, shared by every list.
 *
 * Right-clicking a row that is not in the current selection acts on that row
 * alone, which is what every file manager does — otherwise a stray right-click
 * silently targets a selection you had forgotten about.
 */
export function buildFileMenu(row: FileRow, selectedIds: Set<number>): MenuItem[] {
  const ops = useFileOpsStore.getState();
  const queue = useQueueStore.getState();

  const targets = selectedIds.has(row.id) ? Array.from(selectedIds) : [row.id];
  const many = targets.length > 1;
  const suffix = many ? ` (${targets.length})` : "";
  const blocked = untouchableReason(row.flags);
  const canTouch = isSafeToTouch(row.flags);

  return [
    {
      label: "Open",
      shortcut: "Enter",
      disabled: !canTouch,
      onSelect: () => void ops.openFile(row),
    },
    {
      label: "Open containing folder",
      onSelect: () => void ops.revealFile(row),
    },
    {
      label: `Move to…${suffix}`,
      separatorBefore: true,
      disabled: !canTouch,
      onSelect: () => void ops.moveTo(targets),
    },
    {
      label: `Copy to…${suffix}`,
      disabled: !canTouch,
      onSelect: () => void ops.copyTo(targets),
    },
    {
      label: `Create folder and move…${suffix}`,
      disabled: !canTouch,
      onSelect: () => ops.startNewFolder(targets),
    },
    {
      label: "Rename…",
      shortcut: "F2",
      separatorBefore: true,
      // Renaming is inherently one file at a time.
      disabled: !canTouch || many,
      onSelect: () => ops.startRename(row),
    },
    {
      label: "Copy path",
      shortcut: `${mod}+C`,
      onSelect: () => void ops.copyPath(row),
    },
    {
      label: many ? `Add ${targets.length} to cleanup queue` : "Add to cleanup queue",
      separatorBefore: true,
      disabled: !canTouch,
      onSelect: () => queue.add(targets),
    },
    {
      label: blocked ?? `Move to Recycle Bin${suffix}`,
      shortcut: "Del",
      danger: true,
      disabled: !canTouch,
      onSelect: () => void ops.trash(targets),
    },
  ];
}
