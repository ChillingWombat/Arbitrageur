import React from "react";

export interface ToggleSwitchProps {
  checked: boolean;
  onChange: (checked: boolean) => void;
  disabled?: boolean;
  /** Visible label rendered beside the switch when provided. */
  label?: string;
  /** Accessible name announced by screen readers. Falls back to `label` when omitted. */
  ariaLabel?: string;
  /** Hover tooltip. Falls back to `ariaLabel`/`label`. */
  title?: string;
  size?: "sm" | "md";
  /** Visual color variant: "accent" (#1677ff default) or "emerald" (#10b981). */
  variant?: "accent" | "emerald";
  id?: string;
  /** @deprecated Kept for backwards compatibility; always ignored. */
  showStatusText?: boolean;
}

export const ToggleSwitch: React.FC<ToggleSwitchProps> = ({
  checked,
  onChange,
  disabled = false,
  label,
  ariaLabel,
  title,
  size = "md",
  variant = "accent",
  id,
}) => {
  const isSm = size === "sm";
  const accessibleName = ariaLabel ?? label ?? "Toggle";
  const isEmerald = variant === "emerald";

  return (
    <div className="inline-flex items-center space-x-2 select-none">
      {label && <span className="text-xs font-mono font-medium text-slate-700">{label}</span>}
      <button
        id={id}
        type="button"
        role="switch"
        aria-checked={checked}
        aria-label={accessibleName}
        title={title ?? accessibleName}
        disabled={disabled}
        onClick={(e) => {
          e.preventDefault();
          e.stopPropagation();
          if (!disabled) {
            onChange(!checked);
          }
        }}
        className={`relative inline-flex items-center rounded-full transition-colors duration-200 ease-in-out focus:outline-none focus:ring-2 ${
          isEmerald ? "focus:ring-emerald-500/30" : "focus:ring-accent/30"
        } ${
          isSm ? "w-8 h-[18px]" : "w-10 h-[22px]"
        } ${disabled ? "opacity-40 cursor-not-allowed" : "cursor-pointer"} ${
          checked
            ? isEmerald
              ? "bg-emerald-500 border border-emerald-600 shadow-2xs"
              : "bg-accent border border-accent shadow-xs"
            : "bg-white border border-slate-300 hover:border-slate-400 shadow-xs"
        }`}
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
