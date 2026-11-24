/** Seven screens do not need a router. */

export type Route =
  | "sessions"
  | "swipe"
  | "downloads"
  | "duplicates"
  | "storage"
  | "apps"
  | "queue"
  | "settings";

export interface NavItem {
  route: Route;
  label: string;
  /** Visually separated above, to group the doing from the browsing. */
  groupStart?: boolean;
}

export const NAV_ITEMS: NavItem[] = [
  // The two screens you act in come first; everything else is reference.
  { route: "sessions", label: "Overview" },
  { route: "swipe", label: "Swipe Clean" },
  { route: "downloads", label: "Files", groupStart: true },
  { route: "duplicates", label: "Duplicates" },
  { route: "storage", label: "Storage" },
  { route: "queue", label: "Cleanup Queue" },
  { route: "apps", label: "Installed Apps", groupStart: true },
  { route: "settings", label: "Settings" },
];

export const ROUTES: Route[] = NAV_ITEMS.map((item) => item.route);
