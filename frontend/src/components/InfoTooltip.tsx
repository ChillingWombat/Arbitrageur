import React, { useState } from "react";
import { HelpCircle } from "lucide-react";

export interface InfoTooltipProps {
  text: string;
  className?: string;
  position?: "top" | "bottom";
}

export const InfoTooltip: React.FC<InfoTooltipProps> = ({
  text,
  className = "",
  position = "bottom",
}) => {
  const [isOpen, setIsOpen] = useState(false);

  return (
    <span
      className={`relative inline-flex items-center ml-1 align-middle select-none ${className}`}
      onMouseEnter={() => setIsOpen(true)}
      onMouseLeave={() => setIsOpen(false)}
      onClick={(e) => {
        e.stopPropagation();
        setIsOpen((prev) => !prev);
      }}
    >
      <HelpCircle className="w-3.5 h-3.5 text-slate-400 hover:text-accent transition-colors cursor-help shrink-0" />
      {isOpen && (
        <span
          className={`absolute z-50 w-64 max-w-xs p-2.5 bg-slate-900/95 text-white text-xs font-sans font-normal normal-case tracking-normal whitespace-pre-line rounded-md shadow-xl border border-slate-700/80 pointer-events-none text-left leading-relaxed animate-in fade-in zoom-in-95 duration-150 backdrop-blur-xs ${
            position === "top"
              ? "bottom-full left-1/2 -translate-x-1/2 mb-2"
              : "top-full left-1/2 -translate-x-1/2 mt-2"
          }`}
        >
          {text}
          <span
            className={`absolute left-1/2 -translate-x-1/2 border-4 border-transparent ${
              position === "top"
                ? "top-full border-t-slate-900/95"
                : "bottom-full border-b-slate-900/95"
            }`}
          />
        </span>
      )}
    </span>
  );
};
