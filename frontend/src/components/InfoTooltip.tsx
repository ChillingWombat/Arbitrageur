import React, { useState } from "react";
import { HelpCircle } from "lucide-react";

interface InfoTooltipProps {
  text: string;
  className?: string;
}

export const InfoTooltip: React.FC<InfoTooltipProps> = ({ text, className = "" }) => {
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
        <span className="absolute bottom-full left-1/2 -translate-x-1/2 mb-2 z-50 w-64 max-w-xs p-2.5 bg-slate-900/95 text-white text-[11px] font-sans font-normal rounded-lg shadow-xl border border-slate-700/80 pointer-events-none text-left leading-relaxed animate-in fade-in zoom-in-95 duration-150 backdrop-blur-xs">
          {text}
          <span className="absolute top-full left-1/2 -translate-x-1/2 border-4 border-transparent border-t-slate-900/95" />
        </span>
      )}
    </span>
  );
};
