import React from "react";

export interface SwitchToggleProps {
  checked: boolean;
  onChange: (checked: boolean) => void;
  disabled?: boolean;
  /** Descriptive visible label (e.g. "Auto from history"). Never pass ON/OFF/Enabled status text here. */
  label?: string;
  /** Accessible name for icon-only switches. Falls back to `label`. */
  ariaLabel?: string;
  size?: "sm" | "md";
  /** @deprecated Status text (ON/OFF) is no longer rendered. Kept for backwards compatibility; always ignored. */
  showStatusText?: boolean;
}

export const SwitchToggle: React.FC<SwitchToggleProps> = ({
  checked,
  onChange,
  disabled = false,
  label,
  ariaLabel,
  size = "md",
}) => {
  const isSm = size === "sm";
  const accessibleName = ariaLabel ?? label ?? "Toggle";

  return (
    <div className="inline-flex items-center space-x-2 select-none">
      {label && <span className="text-xs font-mono font-medium text-slate-700">{label}</span>}
      <button
        type="button"
        role="switch"
        aria-checked={checked}
        aria-label={accessibleName}
        disabled={disabled}
        onClick={(e) => {
          e.preventDefault();
          e.stopPropagation();
          if (!disabled) {
            onChange(!checked);
          }
        }}
        className={`relative inline-flex items-center rounded-full transition-colors duration-200 ease-in-out focus:outline-none focus:ring-2 focus:ring-accent/30 ${
          isSm ? "w-8 h-[18px]" : "w-10 h-[22px]"
        } ${disabled ? "opacity-40 cursor-not-allowed" : "cursor-pointer"} ${
          checked
            ? "bg-accent border border-accent shadow-xs"
            : "bg-white border border-slate-300 hover:border-slate-400 shadow-xs"
        }`}
        title={accessibleName}
      >
        <span
          className={`inline-block rounded-full shadow-sm transform transition-transform duration-200 ease-in-out ${
            checked ? "bg-white" : "bg-slate-400"
          } ${
            isSm
              ? checked
                ? "w-3.5 h-3.5 translate-x-4"
                : "w-3.5 h-3.5 translate-x-0.5"
              : checked
              ? "w-4 h-4 translate-x-5"
              : "w-4 h-4 translate-x-0.5"
          }`}
        />
      </button>
    </div>
  );
};
