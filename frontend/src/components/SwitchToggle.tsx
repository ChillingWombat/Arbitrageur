import React from "react";

export interface SwitchToggleProps {
  checked: boolean;
  onChange: (checked: boolean) => void;
  disabled?: boolean;
  label?: string;
  size?: "sm" | "md";
  showStatusText?: boolean;
}

export const SwitchToggle: React.FC<SwitchToggleProps> = ({
  checked,
  onChange,
  disabled = false,
  label,
  size = "md",
  showStatusText = false,
}) => {
  const isSm = size === "sm";

  return (
    <div className="inline-flex items-center space-x-2 select-none">
      {label && <span className="text-xs font-mono font-medium text-slate-700">{label}</span>}
      <button
        type="button"
        role="switch"
        aria-checked={checked}
        disabled={disabled}
        onClick={(e) => {
          e.preventDefault();
          e.stopPropagation();
          if (!disabled) {
            onChange(!checked);
          }
        }}
        className={`relative inline-flex items-center rounded-full transition-colors duration-200 ease-in-out focus:outline-none focus:ring-2 focus:ring-emerald-500/30 ${
          isSm ? "w-8 h-[18px]" : "w-10 h-[22px]"
        } ${disabled ? "opacity-40 cursor-not-allowed" : "cursor-pointer"} ${
          checked
            ? "bg-emerald-500 border border-emerald-600 shadow-2xs"
            : "bg-white border border-slate-300 hover:border-slate-400 shadow-2xs"
        }`}
        title={label ? `${label}: ${checked ? "ON" : "OFF"}` : checked ? "ON" : "OFF"}
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
      {showStatusText && (
        <span
          className={`text-xs font-mono font-bold transition-colors ${
            checked ? "text-emerald-600" : "text-slate-400"
          }`}
        >
          {checked ? "ON" : "OFF"}
        </span>
      )}
    </div>
  );
};
