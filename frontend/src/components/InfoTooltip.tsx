import React, { useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { HelpCircle } from "lucide-react";

export interface InfoTooltipProps {
  text: string;
  className?: string;
  iconClassName?: string;
  position?: "top" | "bottom";
}

const VIEWPORT_MARGIN = 8;
const GAP = 8;

export const InfoTooltip: React.FC<InfoTooltipProps> = ({
  text,
  className = "",
  iconClassName = "text-slate-400 hover:text-accent",
  position = "bottom",
}) => {
  const [isOpen, setIsOpen] = useState(false);
  const triggerRef = useRef<HTMLSpanElement>(null);
  const tipRef = useRef<HTMLSpanElement>(null);
  const [coords, setCoords] = useState<{ top: number; left: number; arrowLeft: number; side: "top" | "bottom" } | null>(null);

  // Render via portal to document.body so no ancestor overflow/transform can
  // clip the tooltip, then clamp horizontally to the viewport and flip
  // vertically when there is not enough room on the requested side.
  useLayoutEffect(() => {
    if (!isOpen) return;
    const trigger = triggerRef.current;
    const tip = tipRef.current;
    if (!trigger || !tip) return;

    const place = () => {
      const tr = trigger.getBoundingClientRect();
      const tw = tip.offsetWidth;
      const th = tip.offsetHeight;
      let side: "top" | "bottom" = position;
      if (
        position === "bottom" &&
        tr.bottom + GAP + th > window.innerHeight - VIEWPORT_MARGIN &&
        tr.top - GAP - th >= VIEWPORT_MARGIN
      ) {
        side = "top";
      } else if (
        position === "top" &&
        tr.top - GAP - th < VIEWPORT_MARGIN
      ) {
        side = "bottom";
      }
      const top =
        side === "bottom" ? tr.bottom + GAP : Math.max(VIEWPORT_MARGIN, tr.top - GAP - th);
      let left = tr.left + tr.width / 2 - tw / 2;
      left = Math.max(VIEWPORT_MARGIN, Math.min(left, window.innerWidth - tw - VIEWPORT_MARGIN));
      const arrowLeft = Math.max(
        10,
        Math.min(tr.left + tr.width / 2 - left, tw - 10),
      );
      setCoords({ top, left, arrowLeft, side });
    };

    place();
    const close = () => setIsOpen(false);
    window.addEventListener("resize", place);
    window.addEventListener("scroll", close, true);
    return () => {
      window.removeEventListener("resize", place);
      window.removeEventListener("scroll", close, true);
    };
  }, [isOpen, position, text]);

  return (
    <span
      ref={triggerRef}
      className={`relative inline-flex items-center ml-1 align-middle select-none ${className}`}
      onMouseEnter={() => setIsOpen(true)}
      onMouseLeave={() => setIsOpen(false)}
      onClick={(e) => {
        e.stopPropagation();
        setIsOpen((prev) => !prev);
      }}
    >
      <HelpCircle className={`w-3.5 h-3.5 transition-colors cursor-help shrink-0 ${iconClassName}`} />
      {isOpen &&
        createPortal(
          <span
            ref={tipRef}
            style={coords ? { top: coords.top, left: coords.left } : { top: -9999, left: -9999 }}
            className="fixed z-[100] w-64 max-w-xs p-2.5 bg-accent-dim/95 text-slate-700 text-xs font-sans font-normal normal-case tracking-normal whitespace-pre-line rounded-md shadow-xl border border-accent-line pointer-events-none text-left leading-relaxed animate-in fade-in zoom-in-95 duration-150 backdrop-blur-xs"
          >
            {text}
            {coords && (
              <span
                style={{ left: coords.arrowLeft }}
                className={`absolute -translate-x-1/2 border-4 border-transparent ${
                  coords.side === "top"
                    ? "top-full border-t-accent-line"
                    : "bottom-full border-b-accent-line"
                }`}
              />
            )}
          </span>,
          document.body,
        )}
    </span>
  );
};
