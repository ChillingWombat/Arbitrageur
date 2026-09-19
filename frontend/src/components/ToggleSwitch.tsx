import React from "react";

interface ToggleSwitchProps {
  checked: boolean;
  onChange: (checked: boolean) => void;
  disabled?: boolean;
  /** Visible text label (legacy). Prefer `ariaLabel` — switches render icon-only with no Enabled/ON/OFF text. */
  label?: string;
  /** Accessible name announced by screen readers. Falls back to `label` when omitted. */
  ariaLabel?: string;
  /** Hover tooltip. Falls back to `ariaLabel`/`label`. Never includes ON/OFF status text. */
  title?: string;
  size?: "sm" | "md";
}

export const ToggleSwitch: React.FC<ToggleSwitchProps> = ({
  checked,
  onChange,
  disabled = false,
  label,
  ariaLabel,
  title,
  size = "md",
}) => {
  const isSm = size === "sm";
  const accessibleName = ariaLabel ?? label ?? (checked ? "Enabled" : "Disabled");
  return (
    <div className="inline-flex items-center space-x-2">
      <button
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
        className={`relative inline-flex items-center rounded-full transition-colors focus:outline-none focus:ring-2 focus:ring-emerald-500/30 ${
          isSm ? "w-8 h-[18px]" : "w-10 h-[22px]"
        } ${disabled ? "opacity-40 cursor-not-allowed" : "cursor-pointer"} ${
          checked ? "bg-emerald-500 border border-emerald-600 shadow-2xs" : "bg-white border border-slate-300 shadow-2xs"
        }`}
      >
        <span
          className={`inline-block rounded-full shadow-2xs transform transition-transform ${
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
      {label && <span className="text-xs font-mono text-slate-700">{label}</span>}
    </div>
  );
};
