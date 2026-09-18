import React, { useEffect, useState } from "react";
import type { SessionProfileDto } from "../types";
import { api } from "../services/api";
import { InfoTooltip } from "./InfoTooltip";
import { SessionBadge } from "./SessionBadge";

interface SessionProfileCardProps {
  symbol: string;
}

const fmtMed = (v: number | null | undefined): string =>
  v == null || !Number.isFinite(v) ? "—" : v.toFixed(1);

const fmtRate = (v: number | null | undefined): string =>
  v == null || !Number.isFinite(v) ? "—" : `${(v * 100).toFixed(0)}%`;

export const SessionProfileCard: React.FC<SessionProfileCardProps> = ({ symbol }) => {
  const [profile, setProfile] = useState<SessionProfileDto | null>(null);
  const [unavailable, setUnavailable] = useState(false);
  const [promoRunning, setPromoRunning] = useState(false);
  const [promoResult, setPromoResult] = useState<string | null>(null);

  const reload = () => {
    let dead = false;
    setProfile(null);
    setUnavailable(false);
    api
      .getSessionProfile(symbol)
      .then((p) => {
        if (!dead) setProfile(p);
      })
      .catch(() => {
        if (!dead) setUnavailable(true);
      });
    return () => {
      dead = true;
    };
  };

  useEffect(reload, [symbol]);

  const handleRecalibrate = () => {
    if (promoRunning) return;
    setPromoRunning(true);
    setPromoResult(null);
    api
      .runSeasonalityPromotion()
      .then((s) => {
        setPromoResult(
          `scanned ${s.symbolsScanned} · anchors ${s.anchorsWritten} · promoted ${s.promoted} · demoted ${s.demoted} · held ${s.held} · skipped ${s.skipped}`,
        );
        reload();
      })
      .catch((err) => {
        setPromoResult(`promotion failed: ${err instanceof Error ? err.message : String(err)}`);
      })
      .finally(() => setPromoRunning(false));
  };

  return (
    <div className="bg-surface p-4 rounded-xl border border-surface-border space-y-2.5 shadow-card">
      <label className="text-xs text-slate-700 font-medium flex items-center justify-between">
        <span className="flex items-center">
          Session Profile
          <InfoTooltip text="Per-session spread aggregates (display only in Phase 1 — nothing here is auto-applied to thresholds or sizing). Provisional sessions read as neutral x1.0 everywhere." />
        </span>
        {profile != null && (
          <SessionBadge
            sessionId={profile.currentSession}
            provisional={profile.provisional}
            allow={profile.sessionAllow}
          />
        )}
      </label>
      {unavailable || profile == null ? (
        <div className="text-xs text-slate-400">
          {unavailable
            ? "Session profiles unavailable — open the desktop app to calibrate."
            : "Loading session aggregates…"}
        </div>
      ) : (
        <div className="space-y-2">
          <div className="flex flex-wrap items-center gap-1.5 text-xs">
            {profile.isWeekend && (
              <span className="px-2 py-0.5 rounded-md border border-warn-border bg-warn-dim text-warn font-medium">
                weekend x0.5
              </span>
            )}
            {profile.inFundingMask && (
              <span className="px-2 py-0.5 rounded-md border border-loss-border bg-loss-dim text-loss font-medium">
                funding stand-down
              </span>
            )}
            <span
              className="px-2 py-0.5 rounded-md border border-surface-border bg-surface-raised text-slate-600 font-medium"
              title="US cash equity hours (Ondo equity-leg gate)"
            >
              US cash {profile.usCashOpen ? "open" : "closed"}
            </span>
          </div>
          {profile.sessions.length === 0 ? (
            <div className="text-xs text-slate-400">
              No calibrated sessions yet — history accumulates while order books are polled.
            </div>
          ) : (
            <table className="w-full text-xs font-mono tabular-nums">
              <thead>
                <tr className="text-slate-400 text-left border-b border-surface-border">
                  <th className="py-1.5 pr-2 font-medium">Session</th>
                  <th className="py-1.5 pr-2 font-medium text-right">Episodes</th>
                  <th className="py-1.5 pr-2 font-medium text-right">Med bps</th>
                  <th className="py-1.5 font-medium text-right">Win</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-surface-border/60">
                {profile.sessions.map((s) => (
                  <tr
                    key={s.sessionId}
                    className={
                      s.sessionId === profile.currentSession
                        ? "text-slate-900 font-bold"
                        : "text-slate-500"
                    }
                  >
                    <td className="py-1 pr-2">
                      {s.sessionId}
                      {s.provisional && <span className="text-slate-400"> *</span>}
                    </td>
                    <td className="py-1 pr-2 text-right">{s.nEpisodes}</td>
                    <td className="py-1 pr-2 text-right">{fmtMed(s.medAbsSpreadBps)}</td>
                    <td className="py-1 text-right">{fmtRate(s.winRate)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
          <div className="text-xs text-slate-400">* provisional — neutral x1.0, flagged</div>
          <div className="flex flex-wrap items-center gap-2 text-xs pt-1">
            <span className="text-slate-500">
              {profile.promotion == null
                ? "Phase-1 baseline — no promoted tables"
                : `${profile.promotion.status}${profile.promotion.rescaled ? " · HAR-rescaled" : ""}${profile.promotion.harR2 != null ? ` · HAR R² ${profile.promotion.harR2.toFixed(2)}` : ""}`}
            </span>
            <button
              type="button"
              onClick={handleRecalibrate}
              disabled={promoRunning}
              className="btn-secondary px-2 py-2 text-xs"
              title="Run the nightly HAR-anchor + walk-forward promotion job now (same code path as the scheduler)"
            >
              {promoRunning ? "Recalibrating…" : "Recalibrate now"}
            </button>
          </div>
          {promoResult != null && (
            <div className="text-xs text-slate-500">{promoResult}</div>
          )}
          {profile.promotion != null && (
            <div className="text-xs text-slate-400" title={profile.promotion.reason}>
              fit {profile.promotion.fitDeltaBps.toFixed(1)} / val {profile.promotion.valDeltaBps.toFixed(1)} bps
            </div>
          )}
        </div>
      )}
    </div>
  );
};

export default SessionProfileCard;
