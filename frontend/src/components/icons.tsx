import React from "react";

/* Lucide-compatible stroke icon: two vertical venue rails with a
   double-headed arrow between them — the Simple Spread strategy mark. */
export const SpreadGapIcon: React.FC<{ className?: string }> = ({ className }) => (
  <svg
    xmlns="http://www.w3.org/2000/svg"
    width="24"
    height="24"
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    strokeWidth="2"
    strokeLinecap="round"
    strokeLinejoin="round"
    className={className}
  >
    <path d="M3.5 4v16" />
    <path d="M20.5 4v16" />
    <path d="M6.5 12h11" />
    <path d="M9 9.5 6.5 12l2.5 2.5" />
    <path d="M15 9.5 17.5 12l-2.5 2.5" />
  </svg>
);
