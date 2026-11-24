interface CheckboxProps {
  checked: boolean;
  onChange: (checked: boolean) => void;
  label: string;
  /** Why it cannot be ticked. Renders disabled with the reason as a tooltip. */
  disabledReason?: string | null;
}

/**
 * A checkbox that refuses to be ticked when the file cannot be acted on, and
 * says why on hover rather than being mysteriously inert.
 */
export function Checkbox({ checked, onChange, label, disabledReason }: CheckboxProps) {
  const disabled = Boolean(disabledReason);
  return (
    <button
      className={`checkbox ${checked ? "on" : ""} ${disabled ? "off-limits" : ""}`}
      role="checkbox"
      aria-checked={checked}
      aria-label={label}
      title={disabledReason ?? label}
      disabled={disabled}
      onClick={(event) => {
        event.stopPropagation();
        if (!disabled) onChange(!checked);
      }}
    >
      {checked ? (
        <svg width="10" height="10" viewBox="0 0 24 24" fill="none" aria-hidden="true">
          <path
            d="M4 12.5l5.5 5.5L20 7"
            stroke="currentColor"
            strokeWidth="3.2"
            strokeLinecap="round"
            strokeLinejoin="round"
          />
        </svg>
      ) : null}
    </button>
  );
}
