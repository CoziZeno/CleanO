import { IconAlert, IconClose, IconInfo } from "./icons";

interface BannerProps {
  tone: "info" | "warn" | "danger";
  title: string;
  text?: string | null;
  /** Shown beneath the text, visually separated from the diagnosis. */
  hint?: string | null;
  onDismiss?: () => void;
}

export function Banner({ tone, title, text, hint, onDismiss }: BannerProps) {
  const Icon = tone === "info" ? IconInfo : IconAlert;
  return (
    <div className={`banner ${tone === "info" ? "" : `banner-${tone}`}`} role="status">
      <span className={`banner-icon ${tone}`}>
        <Icon />
      </span>
      <div className="banner-body">
        <div className="banner-title">{title}</div>
        {text ? <div className="banner-text selectable">{text}</div> : null}
        {hint ? <div className="banner-text">{hint}</div> : null}
      </div>
      {onDismiss ? (
        <button className="btn-ghost" onClick={onDismiss} aria-label="Dismiss" title="Dismiss">
          <IconClose size={14} />
        </button>
      ) : null}
    </div>
  );
}
