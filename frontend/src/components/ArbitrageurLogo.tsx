import React from "react";

interface ArbitrageurLogoProps {
  className?: string;
  size?: number;
}

export const ArbitrageurLogo: React.FC<ArbitrageurLogoProps> = ({
  className = "w-5 h-5 text-accent-strong",
  size = 20,
}) => {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      className={className}
      xmlns="http://www.w3.org/2000/svg"
      aria-label="Arbitrageur Logo"
    >
      {/* Top / Right Arrow forming top semicircle */}
      <path d="M3 12a9 9 0 0 1 9-9 9.75 9.75 0 0 1 6.74 2.74L21 8" />
      <path d="M21 3v5h-5" />

      {/* Bottom / Left Arrow forming bottom semicircle */}
      <path d="M21 12a9 9 0 0 1-9 9 9.75 9.75 0 0 1-6.74-2.74L3 16" />
      <path d="M3 21v-5h5" />

      {/* Dollar Symbol in Center */}
      <line x1="12" y1="6.8" x2="12" y2="17.2" strokeWidth="1.9" />
      <path
        d="M14.5 9.2H10.8a1.6 1.6 0 0 0 0 3.2h2.4a1.6 1.6 0 0 1 0 3.2H9.5"
        strokeWidth="1.9"
      />
    </svg>
  );
};

export default ArbitrageurLogo;
