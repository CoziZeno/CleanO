import { NAV_ITEMS, type Route } from "@/app/nav";
import { useQueueStore } from "@/state/queueStore";
import {
  IconApps,
  IconDownloads,
  IconDuplicates,
  IconOverview,
  IconQueue,
  IconSettings,
  IconSparkle,
  IconStorage,
} from "./icons";

const ICONS: Record<Route, (p: { size?: number }) => JSX.Element> = {
  sessions: IconOverview,
  swipe: IconSparkle,
  downloads: IconDownloads,
  duplicates: IconDuplicates,
  storage: IconStorage,
  apps: IconApps,
  queue: IconQueue,
  settings: IconSettings,
};

interface SidebarProps {
  active: Route;
  onNavigate: (route: Route) => void;
  rootLabel: string | null;
  fileCount: number | null;
}

export function Sidebar({ active, onNavigate, rootLabel, fileCount }: SidebarProps) {
  const queued = useQueueStore((state) => state.selected.size);
  return (
    <nav className="sidebar" aria-label="Main">
      <div className="brand">
        <span className="brand-mark">
          <IconSparkle size={13} />
        </span>
        <span className="brand-name">Cleanup</span>
      </div>

      <div className="nav">
        {NAV_ITEMS.map((item, index) => {
          const Icon = ICONS[item.route];
          return (
            <button
              key={item.route}
              className={`nav-item ${active === item.route ? "active" : ""} ${
                item.groupStart ? "group-start" : ""
              }`}
              onClick={() => onNavigate(item.route)}
              aria-current={active === item.route ? "page" : undefined}
            >
              <Icon size={15} />
              <span className="nav-item-label">{item.label}</span>
              {item.route === "queue" && queued > 0 ? (
                <span className="nav-badge">{queued}</span>
              ) : (
                <span className="nav-key">{index + 1}</span>
              )}
            </button>
          );
        })}
      </div>

      <div className="sidebar-spacer" />

      <div className="sidebar-foot">
        <span className="sidebar-foot-label">Scanning</span>
        <span className="sidebar-foot-value selectable" title={rootLabel ?? undefined}>
          {rootLabel ?? "No folder yet"}
        </span>
        {fileCount !== null ? (
          <span className="sidebar-foot-label">{fileCount.toLocaleString()} files indexed</span>
        ) : null}
      </div>
    </nav>
  );
}
