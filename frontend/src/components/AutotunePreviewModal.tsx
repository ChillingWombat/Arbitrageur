import React, { useMemo, useState } from "react";
import { X, Copy, Check, FlaskConical } from "lucide-react";
import type { AutotuneRecommendationDto, AutotuneApplyResultDto } from "../types";
import { api } from "../services/api";

export interface AutotunePreviewModalProps {
  symbol: string;
  recommendation: AutotuneRecommendationDto | null;
  loading: boolean;
  error: string | null;
  onClose: () => void;
  onApplied?: (result: AutotuneApplyResultDto) => void;
}

export const AUTOTUNE_PARAM_LABELS: Record<string, string> = {
  slidingWindowSecs: "Window",
  targetProfitBps: "Target",
  windowPercentile: "Percentile",
  stepSizeMult: "Step mult",
  maxStepUsd: "Max step",
  maxTickerEquityPct: "Equity %",
  userTargetLeverage: "Leverage",
};

function formatDate(ms: number): string {
  if (!Number.isFinite(ms) || ms <= 0) return "—";
  return new Date(ms).toISOString().slice(0, 10);
}

function formatValue(param: string, v: number | null): string {
  if (v == null || !Number.isFinite(v)) return "—";
  if (param === "slidingWindowSecs") return `${Math.round(v)}s`;
  if (param === "windowPercentile") return `${Math.round(v)}%`;
  if (param === "userTargetLeverage") return `${Math.round(v)}x`;
  if (param === "maxStepUsd") return `$${Math.round(v).toLocaleString("en-US")}`;
  if (param === "maxTickerEquityPct") return `${v}%`;
  if (param === "targetProfitBps") return `${v} bps`;
  return `${v}`;
}

/** Manual-transcription text: READY rows only, `param=value` per line. */
export function buildAutotuneCopyText(rec: AutotuneRecommendationDto): string {
  const lines = [`# AutoTune ${rec.symbol} READY rows (manual transcription or selected apply)`];
  for (const row of rec.rows) {
    if (row.status === "READY" && row.recommended != null) {
      lines.push(`${row.param}=${row.recommended}`);
    }
  }
  return lines.join("\n");
}

export const AutotunePreviewModal: React.FC<AutotunePreviewModalProps> = ({
  symbol,
  recommendation,
  loading,
  error,
  onClose,
  onApplied,
}) => {
  const [copied, setCopied] = useState(false);
  const [selected, setSelected] = useState<Record<string, boolean>>({});
  const [applying, setApplying] = useState(false);
  const [applyError, setApplyError] = useState<string | null>(null);
  const [applyResult, setApplyResult] = useState<AutotuneApplyResultDto | null>(null);

  const readyParams = useMemo(
    () =>
      (recommendation?.rows ?? [])
        .filter((r) => r.status === "READY" && r.recommended != null)
        .map((r) => r.param),
    [recommendation],
  );
  const selectedReady = useMemo(
    () => readyParams.filter((p) => selected[p] === true),
    [readyParams, selected],
  );

  const toggle = (param: string) =>
    setSelected((s) => ({ ...s, [param]: s[param] !== true }));

  const handleCopy = async () => {
    if (!recommendation) return;
    const text = buildAutotuneCopyText(recommendation);
    try {
      await navigator.clipboard.writeText(text);
    } catch {
      const ta = document.createElement("textarea");
      ta.value = text;
      document.body.appendChild(ta);
      ta.select();
      document.execCommand("copy");
      document.body.removeChild(ta);
    }
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  };

  const handleApply = async () => {
    if (selectedReady.length === 0 || applying) return;
    setApplying(true);
    setApplyError(null);
    try {
      const result = await api.applyAutotuneSelected(symbol, selectedReady);
      setApplyResult(result);
      setSelected({});
      if (onApplied) onApplied(result);
    } catch (err) {
      setApplyError(err instanceof Error ? err.message : String(err));
    } finally {
      setApplying(false);
    }
  };

  const banner = recommendation?.banner ?? null;
  const bannerLine = banner
    ? `Walk-forward 70/30 · fit ${formatDate(banner.fit_from_ms)}→${formatDate(banner.fit_to_ms)} · validate ${formatDate(banner.validate_from_ms)}→${formatDate(banner.validate_to_ms)} · regime ${banner.regime_verdict} · READY ${banner.ready_count}/${banner.total}`
    : "No recommendation loaded.";
  const couplingLine =
    recommendation != null &&
    Number.isFinite(recommendation.session_tau_scale) &&
    Number.isFinite(recommendation.session_sigma_scale)
      ? `Session coupling ×τ ${recommendation.session_tau_scale.toFixed(2)} · ×σ ${recommendation.session_sigma_scale.toFixed(2)}`
      : null;

  return (
    <div
      className="fixed inset-0 z-[60] bg-slate-900/50 backdrop-blur-sm flex items-center justify-center p-3 sm:p-6 select-none animate-in fade-in duration-150"
      onClick={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div className="bg-surface border border-surface-border rounded-2xl w-[860px] max-w-[96vw] max-h-[92vh] flex flex-col shadow-2xl overflow-hidden animate-in zoom-in-95 duration-150">
        <div className="flex items-center justify-between px-6 py-3.5 border-b border-surface-border shrink-0">
          <div className="flex items-center space-x-3">
            <div className="w-8 h-8 rounded-lg bg-accent-dim border border-accent-line flex items-center justify-center text-accent shadow-2xs">
              <FlaskConical className="w-4 h-4" />
            </div>
            <div>
              <h2 className="text-sm font-bold font-sans tracking-tight text-slate-900 leading-tight">
                AutoTune Preview · {symbol}
              </h2>
              <p className="text-[11px] font-sans text-slate-500">
                One-click apply writes READY rows only — HELD rows are never applied
              </p>
            </div>
          </div>
          <button
            type="button"
            onClick={onClose}
            className="p-1.5 rounded-lg text-slate-400 hover:text-slate-700 hover:bg-slate-100 transition border border-transparent hover:border-surface-border cursor-pointer"
            title="Close preview (ESC)"
          >
            <X className="w-4 h-4" />
          </button>
        </div>

        <div className="px-6 py-2.5 border-b border-surface-border bg-slate-50/70 shrink-0">
          <p className="text-[11px] font-mono text-slate-600">{bannerLine}</p>
          {couplingLine != null && (
            <p className="text-[11px] font-mono text-slate-500">{couplingLine}</p>
          )}
          {recommendation?.readiness && (
            <div className="mt-1.5 text-[11px] font-mono">
              <p className="text-slate-500">
                evidence: {recommendation.readiness.venues_seen.length} venue
                {recommendation.readiness.venues_seen.length === 1 ? "" : "s"} (
                {recommendation.readiness.venues_seen.join(", ") || "none"}) ·{" "}
                {(recommendation.readiness.history_span_secs / 3600).toFixed(1)}h span ·{" "}
                {recommendation.readiness.pair_observations}/
                {recommendation.readiness.min_observations_required} pair obs ·{" "}
                {(recommendation.readiness.multi_venue_coverage * 100).toFixed(0)}% multi-venue
                coverage
                {recommendation.readiness.measured_slippage_bps != null && (
                  <>
                    {" · "}slippage{" "}
                    {recommendation.readiness.measured_slippage_bps >= 0 ? "+" : ""}
                    {recommendation.readiness.measured_slippage_bps.toFixed(1)}bps measured
                  </>
                )}
              </p>
              {recommendation.readiness.blockers.map((b, i) => (
                <p key={i} className="text-amber-600">· {b}</p>
              ))}
            </div>
          )}
        </div>

        <div className="flex-1 overflow-y-auto p-4 font-mono">
          {loading && <p className="text-xs text-slate-500">Computing walk-forward recommendation…</p>}
          {error && <p className="text-xs text-loss">AutoTune failed: {error}</p>}
          {applyError && <p className="text-xs text-loss">Apply failed: {applyError}</p>}
          {applyResult && (
            <p className="text-xs text-profit font-semibold">
              Applied {applyResult.applied.length} row(s)
              {applyResult.audit_id != null ? ` · audit #${applyResult.audit_id}` : ""}
              {applyResult.skipped_held.length > 0
                ? ` · skipped HELD: ${applyResult.skipped_held.join(", ")}`
                : ""}
            </p>
          )}
          {!loading && !error && recommendation && (
            <table className="w-full text-[11px] border-collapse">
              <thead>
                <tr className="text-left text-slate-500 border-b border-surface-border">
                  <th className="py-1.5 pr-2 font-semibold">Apply</th>
                  <th className="py-1.5 pr-2 font-semibold">Param</th>
                  <th className="py-1.5 pr-2 font-semibold">Current</th>
                  <th className="py-1.5 pr-2 font-semibold">Recommended</th>
                  <th className="py-1.5 pr-2 font-semibold">Status</th>
                  <th className="py-1.5 pr-2 font-semibold">Fit</th>
                  <th className="py-1.5 pr-2 font-semibold">Validated</th>
                  <th className="py-1.5 pr-2 font-semibold">Rationale</th>
                </tr>
              </thead>
              <tbody>
                {recommendation.rows.map((row) => {
                  const checkable = row.status === "READY" && row.recommended != null;
                  return (
                    <tr key={row.param} className="border-b border-surface-border/60 align-top">
                      <td className="py-1.5 pr-2">
                        <input
                          type="checkbox"
                          checked={checkable && selected[row.param] === true}
                          disabled={!checkable || applying}
                          onChange={() => toggle(row.param)}
                          title={
                            checkable
                              ? `Apply ${row.param}`
                              : "HELD rows are never applied"
                          }
                        />
                      </td>
                      <td className="py-1.5 pr-2 font-bold text-slate-800 whitespace-nowrap">
                        {AUTOTUNE_PARAM_LABELS[row.param] ?? row.param}
                        <span className="block font-normal text-slate-400 text-[10px]">{row.formula_id}</span>
                      </td>
                      <td className="py-1.5 pr-2 text-slate-600 whitespace-nowrap">
                        {formatValue(row.param, row.current)}
                      </td>
                      <td className="py-1.5 pr-2 font-bold text-slate-900 whitespace-nowrap">
                        {formatValue(row.param, row.recommended)}
                      </td>
                      <td className="py-1.5 pr-2 whitespace-nowrap">
                        <span
                          className={`inline-block px-1.5 py-0.5 rounded text-[10px] font-bold ${
                            row.status === "READY"
                              ? "bg-profit-dim text-profit border border-profit-border"
                              : "bg-surface-raised text-slate-600 border border-surface-border"
                          }`}
                        >
                          {row.status}
                        </span>
                        <span className="block text-slate-400 text-[10px]">{row.confidence}</span>
                      </td>
                      <td className="py-1.5 pr-2 text-slate-500 max-w-44 break-words">{row.fit_metric}</td>
                      <td className="py-1.5 pr-2 text-slate-500 max-w-44 break-words">
                        {row.validated_metric}
                      </td>
                      <td className="py-1.5 pr-2 text-slate-500 max-w-56 break-words">
                        {row.rationale}
                        <span className="block text-slate-400 text-[10px]">n={row.n}</span>
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          )}
        </div>

        <div className="px-6 py-3.5 border-t border-surface-border bg-surface flex items-center justify-end space-x-3 shrink-0 font-mono text-xs">
          <button
            type="button"
            onClick={handleCopy}
            disabled={!recommendation || loading}
            className="flex items-center space-x-1.5 px-2.5 py-2.5 bg-surface hover:bg-surface-raised text-slate-700 border border-surface-border rounded-lg font-medium transition cursor-pointer disabled:opacity-50"
            title="Copy READY values as param=value lines for manual transcription"
          >
            {copied ? <Check className="w-4 h-4" /> : <Copy className="w-4 h-4" />}
            <span>{copied ? "Copied!" : "Copy values"}</span>
          </button>
          <button
            type="button"
            onClick={handleApply}
            disabled={selectedReady.length === 0 || applying || loading}
            className="px-2.5 py-2.5 bg-emerald-600 hover:bg-emerald-700 text-white rounded-lg font-semibold transition cursor-pointer disabled:opacity-50"
            title="Write selected READY rows through saveStrategyParams/saveTickerStrategyConfig with an audit line"
          >
            {applying ? "Applying…" : `Apply selected (${selectedReady.length})`}
          </button>
          <button
            type="button"
            onClick={onClose}
            className="px-2.5 py-2.5 bg-accent hover:bg-blue-600 text-white rounded-lg font-semibold transition cursor-pointer"
          >
            Close
          </button>
        </div>
      </div>
    </div>
  );
};
