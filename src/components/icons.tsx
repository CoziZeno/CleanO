/**
 * Inline 16px stroke icons.
 *
 * Hand-written rather than an icon package: this is the entire set the app
 * needs, and it keeps the bundle free of a dependency that would ship a
 * thousand glyphs to use twelve.
 */

import type { ReactNode } from "react";

interface IconProps {
  size?: number;
  className?: string;
}

function Svg({ size = 16, className, children }: IconProps & { children: ReactNode }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.8"
      strokeLinecap="round"
      strokeLinejoin="round"
      className={className}
      aria-hidden="true"
      focusable="false"
    >
      {children}
    </svg>
  );
}

export const IconOverview = (p: IconProps) => (
  <Svg {...p}>
    <rect x="3" y="3" width="7.5" height="7.5" rx="1.5" />
    <rect x="13.5" y="3" width="7.5" height="7.5" rx="1.5" />
    <rect x="3" y="13.5" width="7.5" height="7.5" rx="1.5" />
    <rect x="13.5" y="13.5" width="7.5" height="7.5" rx="1.5" />
  </Svg>
);

export const IconDownloads = (p: IconProps) => (
  <Svg {...p}>
    <path d="M12 3v11" />
    <path d="m7.5 10 4.5 4.5 4.5-4.5" />
    <path d="M4 17.5v1.5a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-1.5" />
  </Svg>
);

export const IconDuplicates = (p: IconProps) => (
  <Svg {...p}>
    <rect x="9" y="9" width="12" height="12" rx="2" />
    <path d="M15 5.5A2.5 2.5 0 0 0 12.5 3H5.5A2.5 2.5 0 0 0 3 5.5v7A2.5 2.5 0 0 0 5.5 15" />
  </Svg>
);

export const IconStorage = (p: IconProps) => (
  <Svg {...p}>
    <rect x="2.5" y="4" width="19" height="7" rx="2" />
    <rect x="2.5" y="13" width="19" height="7" rx="2" />
    <path d="M6.5 7.5h.01M6.5 16.5h.01" />
  </Svg>
);

export const IconQueue = (p: IconProps) => (
  <Svg {...p}>
    <path d="M3.5 6.5h17" />
    <path d="M9.5 3.5h5" />
    <path d="M5.5 6.5l.9 12.1a2 2 0 0 0 2 1.9h7.2a2 2 0 0 0 2-1.9l.9-12.1" />
    <path d="M10 11v5.5M14 11v5.5" />
  </Svg>
);

export const IconSettings = (p: IconProps) => (
  <Svg {...p}>
    <path d="M4 7h16M4 12h16M4 17h16" />
    <circle cx="9" cy="7" r="2" fill="currentColor" stroke="none" />
    <circle cx="15" cy="12" r="2" fill="currentColor" stroke="none" />
    <circle cx="8" cy="17" r="2" fill="currentColor" stroke="none" />
  </Svg>
);

export const IconSearch = (p: IconProps) => (
  <Svg {...p}>
    <circle cx="10.5" cy="10.5" r="6.5" />
    <path d="m15.5 15.5 4.5 4.5" />
  </Svg>
);

export const IconClose = (p: IconProps) => (
  <Svg {...p}>
    <path d="M5.5 5.5l13 13M18.5 5.5l-13 13" />
  </Svg>
);

export const IconRefresh = (p: IconProps) => (
  <Svg {...p}>
    <path d="M20 11A8 8 0 0 0 6.3 6.3L4 8.5" />
    <path d="M4 4.5V9h4.5" />
    <path d="M4 13a8 8 0 0 0 13.7 4.7L20 15.5" />
    <path d="M20 19.5V15h-4.5" />
  </Svg>
);

export const IconFolder = (p: IconProps) => (
  <Svg {...p}>
    <path d="M3.5 7a2 2 0 0 1 2-2h3.3a2 2 0 0 1 1.5.7l1.2 1.3h7A2 2 0 0 1 20.5 9v8a2 2 0 0 1-2 2h-13a2 2 0 0 1-2-2V7Z" />
  </Svg>
);

export const IconAlert = (p: IconProps) => (
  <Svg {...p}>
    <path d="M12 4.5 2.8 20h18.4L12 4.5Z" />
    <path d="M12 10v4.2M12 17.3h.01" />
  </Svg>
);

export const IconInfo = (p: IconProps) => (
  <Svg {...p}>
    <circle cx="12" cy="12" r="9" />
    <path d="M12 11v5.5M12 7.8h.01" />
  </Svg>
);

export const IconSparkle = (p: IconProps) => (
  <Svg {...p}>
    <path d="M12 3.5l1.8 4.9 4.9 1.8-4.9 1.8L12 16.9l-1.8-4.9L5.3 10.2l4.9-1.8L12 3.5Z" />
    <path d="M18.5 16.5l.7 1.8 1.8.7-1.8.7-.7 1.8-.7-1.8-1.8-.7 1.8-.7.7-1.8Z" />
  </Svg>
);

export const IconArrowUp = (p: IconProps) => (
  <Svg {...p}>
    <path d="M12 19V6" />
    <path d="m6.5 11.5 5.5-5.5 5.5 5.5" />
  </Svg>
);

export const IconArrowDown = (p: IconProps) => (
  <Svg {...p}>
    <path d="M12 5v13" />
    <path d="m6.5 12.5 5.5 5.5 5.5-5.5" />
  </Svg>
);

export const IconReveal = (p: IconProps) => (
  <Svg {...p}>
    <path d="M13 4.5h6.5V11" />
    <path d="M19.5 4.5 11 13" />
    <path d="M18 14.5v4a1.5 1.5 0 0 1-1.5 1.5h-11A1.5 1.5 0 0 1 4 18.5v-11A1.5 1.5 0 0 1 5.5 6h4" />
  </Svg>
);

export const IconApps = (p: IconProps) => (
  <Svg {...p}>
    <rect x="3.5" y="3.5" width="7" height="7" rx="1.8" />
    <rect x="13.5" y="3.5" width="7" height="7" rx="3.5" />
    <rect x="3.5" y="13.5" width="7" height="7" rx="3.5" />
    <rect x="13.5" y="13.5" width="7" height="7" rx="1.8" />
  </Svg>
);

export const IconCloud = (p: IconProps) => (
  <Svg {...p}>
    <path d="M7 18.5h9.5a3.5 3.5 0 0 0 .4-7A5.5 5.5 0 0 0 6.4 10 3.75 3.75 0 0 0 7 18.5Z" />
  </Svg>
);
