import React from "react";
import { InfoTooltip } from "./InfoTooltip";

/** One labelled metric cell inside a DetailSection grid. `tip` renders a
 *  trailing `?` affordance with the metric's definition. */
export const DetailField: React.FC<{
  label: string;
  value: React.ReactNode;
  title?: string;
  tip?: string;
}> = ({ label, value, title, tip }) => (
  <div className="space-y-0.5" title={title}>
    <div className="text-[11px] font-sans text-slate-400 font-medium flex items-center">
      {label}
      {tip && <InfoTooltip text={tip} position="top" />}
    </div>
    <div className="text-sm font-bold font-sans text-slate-900 tabular-nums">{value}</div>
  </div>
);

/** Titled metric group — the shared section container every opportunity
 *  detail modal uses. */
export const DetailSection: React.FC<{ title: string; children: React.ReactNode }> = ({
  title,
  children,
}) => (
  <div className="space-y-2">
    <div className="text-[11px] font-sans font-bold uppercase tracking-wider text-slate-400">
      {title}
    </div>
    <div className="grid grid-cols-2 sm:grid-cols-3 gap-x-4 gap-y-3">{children}</div>
  </div>
);
