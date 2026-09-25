import React, { useState } from "react";
import {
  Server,
  HardDrive,
  Database,
  Download,
  Play,
  RefreshCw,
  CheckCircle2,
  AlertTriangle,
  Code,
  Terminal,
  Shield,
  Clock,
  ChevronDown,
  ChevronUp,
} from "lucide-react";
import { api } from "../services/api";

interface PriceRetriverPanelProps {
  onNotify?: (msg: string, type?: "success" | "error" | "info") => void;
}

interface NodeStatus {
  status: string;
  uptime_secs: number;
  data_dir: string;
  total_stored_bytes: number;
  max_allowed_bytes: number;
  disk_free_bytes: number;
  total_files: number;
  active_collectors: Record<string, boolean>;
  message_rates: Record<string, number>;
}

interface Partition {
  venue: string;
  symbol: string;
  date: string;
  hour: string;
  file_size_bytes: number;
  relative_path: string;
}

const getStorageItem = (key: string, fallback: string): string => {
  if (typeof window !== "undefined" && typeof window.localStorage !== "undefined") {
    try {
      return window.localStorage.getItem(key) || fallback;
    } catch {
      return fallback;
    }
  }
  return fallback;
};

const setStorageItem = (key: string, value: string): void => {
  if (typeof window !== "undefined" && typeof window.localStorage !== "undefined") {
    try {
      window.localStorage.setItem(key, value);
    } catch {
      // ignore
    }
  }
};

export const PriceRetriverPanel: React.FC<PriceRetriverPanelProps> = ({ onNotify }) => {
  // Connection states
  const [nodeUrl, setNodeUrl] = useState<string>(() => {
    return getStorageItem("priceretriver_url", "http://127.0.0.1:8888");
  });
  const [token, setToken] = useState<string>(() => {
    return getStorageItem("priceretriver_token", "");
  });
  const [isConnected, setIsConnected] = useState<boolean>(false);
  const [isConnecting, setIsConnecting] = useState<boolean>(false);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  // Live node data
  const [status, setStatus] = useState<NodeStatus | null>(null);
  const [partitions, setPartitions] = useState<Partition[]>([]);
  const [loadingPartitions, setLoadingPartitions] = useState<boolean>(false);

  // Download state
  const [downloadingVenue, setDownloadingVenue] = useState<string>("");
  const [downloadingSymbol] = useState<string>("");
  const [isDownloading, setIsDownloading] = useState<boolean>(false);
  const [downloadMsg, setDownloadMsg] = useState<string | null>(null);

  // Preprocessing / Custom script state
  const [activePipelineTab, setActivePipelineTab] = useState<"prebuilt" | "custom">("prebuilt");
  const [resampleInterval, setResampleInterval] = useState<string>("1m");
  const [resampleFormat, setResampleFormat] = useState<string>("csv");
  const [selectedPartition, setSelectedPartition] = useState<string>("");
  const [isProcessing, setIsProcessing] = useState<boolean>(false);
  const [processResult, setProcessResult] = useState<string | null>(null);

  // Custom Python Script Studio
  const [customScript, setCustomScript] = useState<string>(
    `#!/usr/bin/env python3\nimport sys, json\nprint("[Arbitrageur Custom Preprocess] Processing stream...")\n# Process input file and output features\n`
  );
  const [customLanguage] = useState<string>("python");
  const [customOutput, setCustomOutput] = useState<{ stdout: string; stderr: string; success: boolean; duration_ms: number } | null>(null);

  // Guide accordion
  const [showGuide, setShowGuide] = useState<boolean>(false);

  // Connect & fetch status
  const handleConnect = async () => {
    setIsConnecting(true);
    setErrorMsg(null);
    try {
      setStorageItem("priceretriver_url", nodeUrl);
      setStorageItem("priceretriver_token", token);

      const res = await api.priceRetriverStatus(nodeUrl, token);
      setStatus(res);
      setIsConnected(true);
      if (onNotify) onNotify("Connected to PriceRetriver Node successfully!", "success");
      void fetchPartitions();
    } catch (err) {
      setIsConnected(false);
      const msg = err instanceof Error ? err.message : String(err);
      setErrorMsg(`Failed to connect to PriceRetriver node: ${msg}`);
      if (onNotify) onNotify(`Connection failed: ${msg}`, "error");
    } finally {
      setIsConnecting(false);
    }
  };

  const fetchPartitions = async () => {
    setLoadingPartitions(true);
    try {
      const res = await api.priceRetriverListPartitions(nodeUrl, token);
      if (Array.isArray(res)) {
        setPartitions(res);
        if (res.length > 0 && !selectedPartition) {
          setSelectedPartition(res[0].relative_path);
        }
      }
    } catch {
      // Ignore background partition list error
    } finally {
      setLoadingPartitions(false);
    }
  };

  // Trigger download
  const handleDownload = async () => {
    setIsDownloading(true);
    setDownloadMsg(null);
    try {
      const fileName = `priceretriver_${downloadingVenue || "all"}_${Date.now()}.tar.gz`;
      const bytes = await api.priceRetriverDownloadData(
        nodeUrl,
        token,
        downloadingVenue || undefined,
        downloadingSymbol || undefined,
        fileName
      );
      const sizeMb = (bytes / (1024 * 1024)).toFixed(2);
      setDownloadMsg(`Successfully downloaded ${sizeMb} MB archive to ${fileName}`);
      if (onNotify) onNotify(`Downloaded ${sizeMb} MB to ${fileName}`, "success");
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      setDownloadMsg(`Download failed: ${msg}`);
      if (onNotify) onNotify(`Download error: ${msg}`, "error");
    } finally {
      setIsDownloading(false);
    }
  };

  // Run prebuilt pipeline
  const handleRunPipeline = async () => {
    if (!selectedPartition) {
      alert("Please select a target partition first.");
      return;
    }
    setIsProcessing(true);
    setProcessResult(null);
    try {
      const res = await api.priceRetriverRunPipeline(
        nodeUrl,
        token,
        selectedPartition,
        `resample_${resampleInterval}`,
        resampleFormat
      );
      setProcessResult(JSON.stringify(res, null, 2));
      if (onNotify) onNotify("Preprocessing pipeline executed successfully!", "success");
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      setProcessResult(`Error: ${msg}`);
      if (onNotify) onNotify(`Pipeline error: ${msg}`, "error");
    } finally {
      setIsProcessing(false);
    }
  };

  // Run custom script
  const handleRunCustomScript = async () => {
    setIsProcessing(true);
    setCustomOutput(null);
    try {
      const res = await api.priceRetriverRunCustomScript(
        nodeUrl,
        token,
        customScript,
        customLanguage,
        selectedPartition || undefined
      );
      setCustomOutput(res);
      if (onNotify) onNotify("Custom script executed on remote VPS!", "success");
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      setCustomOutput({
        stdout: "",
        stderr: `Execution failed: ${msg}`,
        success: false,
        duration_ms: 0,
      });
      if (onNotify) onNotify(`Script error: ${msg}`, "error");
    } finally {
      setIsProcessing(false);
    }
  };

  // Helpers
  const formatBytes = (bytes: number): string => {
    if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
    if (bytes >= 1073741824) return `${(bytes / 1073741824).toFixed(2)} GB`;
    if (bytes >= 1048576) return `${(bytes / 1048576).toFixed(2)} MB`;
    return `${(bytes / 1024).toFixed(1)} KB`;
  };

  const formatUptime = (secs: number): string => {
    const hours = Math.floor(secs / 3600);
    const mins = Math.floor((secs % 3600) / 60);
    return `${hours}h ${mins}m`;
  };

  const storagePct = status && status.max_allowed_bytes > 0
    ? Math.min(100, Math.round((status.total_stored_bytes / status.max_allowed_bytes) * 100))
    : 0;

  return (
    <div className="space-y-4 max-w-5xl mx-auto text-xs font-mono">
      {/* Top Banner / Card */}
      <div className="p-4 bg-surface border border-surface-border rounded-xl shadow-2xs space-y-3">
        <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-3 border-b border-surface-border pb-3">
          <div className="flex items-center space-x-3">
            <div className="w-10 h-10 rounded-lg bg-accent-dim border border-accent-line flex items-center justify-center text-accent-strong shrink-0">
              <Server className="w-5 h-5" />
            </div>
            <div>
              <div className="flex items-center space-x-2">
                <h3 className="text-sm font-bold text-slate-900 tracking-tight">
                  PriceRetriver 24/7 VPS Data Ingestion Node
                </h3>
                <span
                  className={`inline-flex items-center px-2 py-0.5 rounded-full text-[10px] font-bold ${
                    isConnected
                      ? "bg-profit-dim text-profit border border-profit-border"
                      : "bg-slate-100 text-slate-600 border border-surface-border"
                  }`}
                >
                  <span
                    className={`w-1.5 h-1.5 rounded-full mr-1.5 ${
                      isConnected ? "bg-profit animate-pulse" : "bg-slate-400"
                    }`}
                  />
                  {isConnected ? "ONLINE & STREAMING" : "DISCONNECTED"}
                </span>
              </div>
              <p className="text-xs text-slate-500 font-sans mt-0.5">
                Continuous market data retriever running on free/budget VPS with rolling storage and remote export.
              </p>
            </div>
          </div>

          <div className="flex items-center space-x-2">
            <button
              type="button"
              onClick={() => setShowGuide(!showGuide)}
              className="px-2.5 py-1.5 rounded-lg border border-surface-border bg-surface hover:bg-slate-50 text-slate-700 text-xs flex items-center space-x-1.5 transition cursor-pointer"
            >
              <Terminal className="w-3.5 h-3.5 text-slate-500" />
              <span>VPS Setup Guide</span>
              {showGuide ? <ChevronUp className="w-3 h-3" /> : <ChevronDown className="w-3 h-3" />}
            </button>
          </div>
        </div>

        {/* Collapsible VPS Quickstart Instructions */}
        {showGuide && (
          <div className="p-3 bg-slate-900 text-slate-200 rounded-lg space-y-2 text-xs font-mono">
            <div className="flex items-center justify-between text-accent">
              <span className="font-bold flex items-center space-x-1.5">
                <Terminal className="w-3.5 h-3.5" />
                <span>Deploy PriceRetriver on your VPS:</span>
              </span>
              <span className="text-[10px] text-slate-400">Ubuntu 22.04+ / Debian / Docker</span>
            </div>
            <div className="space-y-1 text-slate-300">
              <p className="text-slate-400"># Option A: One-line automated production systemd installer</p>
              <div className="p-2 bg-slate-950 rounded text-emerald-400 select-all overflow-x-auto">
                <code>sudo bash scripts/install-vps.sh</code>
              </div>
              <p className="text-slate-400 pt-1"># Option B: Run via Docker Compose</p>
              <div className="p-2 bg-slate-950 rounded text-emerald-400 select-all overflow-x-auto">
                <code>docker compose up -d</code>
              </div>
            </div>
          </div>
        )}

        {/* Node Endpoint & Token Input Form */}
        <div className="grid grid-cols-1 sm:grid-cols-12 gap-3 items-end pt-1">
          <div className="sm:col-span-6 space-y-1">
            <label className="text-slate-700 font-bold text-xs flex items-center space-x-1.5">
              <Server className="w-3.5 h-3.5 text-slate-400" />
              <span>Node REST & WS URL</span>
            </label>
            <input
              type="text"
              value={nodeUrl}
              onChange={(e) => setNodeUrl(e.target.value)}
              placeholder="http://198.51.100.24:8888 or http://localhost:8888"
              className="w-full px-3 py-2 bg-surface-raised border border-surface-border rounded-lg text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent text-xs"
            />
          </div>

          <div className="sm:col-span-4 space-y-1">
            <label className="text-slate-700 font-bold text-xs flex items-center space-x-1.5">
              <Shield className="w-3.5 h-3.5 text-slate-400" />
              <span>API Authentication Token</span>
            </label>
            <input
              type="password"
              value={token}
              onChange={(e) => setToken(e.target.value)}
              placeholder="Bearer secret token..."
              className="w-full px-3 py-2 bg-surface-raised border border-surface-border rounded-lg text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent text-xs"
            />
          </div>

          <div className="sm:col-span-2">
            <button
              type="button"
              onClick={handleConnect}
              disabled={isConnecting || !nodeUrl}
              className="w-full py-2 px-3 bg-accent hover:bg-accent-strong disabled:opacity-50 text-white font-bold rounded-lg transition shadow-2xs flex items-center justify-center space-x-1.5 cursor-pointer text-xs"
            >
              <RefreshCw className={`w-3.5 h-3.5 ${isConnecting ? "animate-spin" : ""}`} />
              <span>{isConnecting ? "Connecting..." : isConnected ? "Refresh" : "Connect"}</span>
            </button>
          </div>
        </div>

        {errorMsg && (
          <div className="p-2.5 bg-loss-dim border border-loss-border text-loss rounded-lg text-xs flex items-center space-x-2">
            <AlertTriangle className="w-4 h-4 shrink-0" />
            <span>{errorMsg}</span>
          </div>
        )}
      </div>

      {/* Node Metrics & Capacity Display (When Connected) */}
      {isConnected && status && (
        <div className="grid grid-cols-1 sm:grid-cols-3 gap-3">
          {/* Storage Capacity Gauge */}
          <div className="p-3.5 bg-surface border border-surface-border rounded-xl shadow-2xs space-y-2">
            <div className="flex items-center justify-between text-slate-700">
              <span className="font-bold flex items-center space-x-1.5">
                <HardDrive className="w-3.5 h-3.5 text-accent-strong" />
                <span>Rolling Disk Quota</span>
              </span>
              <span className="text-[11px] font-bold text-slate-900">{storagePct}%</span>
            </div>

            <div className="w-full bg-slate-100 rounded-full h-2 overflow-hidden border border-surface-border">
              <div
                className={`h-full rounded-full transition-all duration-500 ${
                  storagePct > 85 ? "bg-loss" : storagePct > 65 ? "bg-warn-strong" : "bg-accent"
                }`}
                style={{ width: `${storagePct}%` }}
              />
            </div>

            <div className="flex items-center justify-between text-[11px] text-slate-500 pt-1">
              <span>Used: {formatBytes(status.total_stored_bytes)}</span>
              <span>Cap: {formatBytes(status.max_allowed_bytes)}</span>
            </div>
          </div>

          {/* VPS Host Disk Health */}
          <div className="p-3.5 bg-surface border border-surface-border rounded-xl shadow-2xs space-y-2">
            <div className="flex items-center justify-between text-slate-700">
              <span className="font-bold flex items-center space-x-1.5">
                <Database className="w-3.5 h-3.5 text-profit" />
                <span>VPS Host Free Disk</span>
              </span>
              <span className="text-profit font-bold text-[11px]">Healthy</span>
            </div>
            <div className="text-lg font-bold text-slate-900">
              {formatBytes(status.disk_free_bytes)}
            </div>
            <div className="text-[11px] text-slate-500">
              {status.total_files} active compressed partitions
            </div>
          </div>

          {/* Node Uptime & Active Venues */}
          <div className="p-3.5 bg-surface border border-surface-border rounded-xl shadow-2xs space-y-2">
            <div className="flex items-center justify-between text-slate-700">
              <span className="font-bold flex items-center space-x-1.5">
                <Clock className="w-3.5 h-3.5 text-accent-strong" />
                <span>Node Uptime</span>
              </span>
              <span className="text-slate-900 font-bold text-[11px]">{formatUptime(status.uptime_secs)}</span>
            </div>
            <div className="flex items-center space-x-2 pt-1">
              {Object.entries(status.active_collectors).map(([venue, active]) => (
                <span
                  key={venue}
                  className={`px-1.5 py-0.5 rounded text-[10px] uppercase font-bold ${
                    active
                      ? "bg-profit-dim text-profit border border-profit-border"
                      : "bg-slate-100 text-slate-400 border border-surface-border"
                  }`}
                  title={`${venue}: ${active ? "Streaming" : "Stopped"}`}
                >
                  {venue}
                </span>
              ))}
            </div>
            <div className="text-[11px] text-slate-500">
              Ingress path: {status.data_dir}
            </div>
          </div>
        </div>
      )}

      {/* Partitions & Remote Data Downloader */}
      {isConnected && (
        <div className="p-4 bg-surface border border-surface-border rounded-xl shadow-2xs space-y-3">
          <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-3 border-b border-surface-border pb-3">
            <div>
              <h4 className="font-bold text-slate-900 flex items-center space-x-2">
                <Download className="w-4 h-4 text-accent-strong" />
                <span>Remote Market Data Downloader</span>
              </h4>
              <p className="text-xs text-slate-500 font-sans mt-0.5">
                Download stored Zstandard market archives (.tar.gz) from your VPS directly to your local computer.
              </p>
            </div>

            <div className="flex items-center space-x-2">
              <select
                value={downloadingVenue}
                onChange={(e) => setDownloadingVenue(e.target.value)}
                className="px-2.5 py-1.5 bg-surface-raised border border-surface-border rounded-lg text-slate-900 text-xs"
              >
                <option value="">All Venues</option>
                <option value="binance">Binance</option>
                <option value="hyperliquid">Hyperliquid</option>
                <option value="okx">OKX</option>
                <option value="coinbase">Coinbase</option>
                <option value="alpaca">Alpaca</option>
                <option value="kraken">Kraken</option>
              </select>

              <button
                type="button"
                onClick={handleDownload}
                disabled={isDownloading}
                className="px-3 py-1.5 bg-accent hover:bg-accent-strong text-white font-bold rounded-lg transition shadow-2xs flex items-center space-x-1.5 cursor-pointer text-xs"
              >
                <Download className={`w-3.5 h-3.5 ${isDownloading ? "animate-bounce" : ""}`} />
                <span>{isDownloading ? "Downloading..." : "Download Dataset (.tar.gz)"}</span>
              </button>
            </div>
          </div>

          {downloadMsg && (
            <div className="p-2.5 bg-profit-dim border border-profit-border text-profit rounded-lg text-xs flex items-center space-x-2">
              <CheckCircle2 className="w-4 h-4 shrink-0" />
              <span>{downloadMsg}</span>
            </div>
          )}

          {/* Partitions Table */}
          <div className="space-y-2">
            <div className="flex items-center justify-between text-xs text-slate-600 font-bold">
              <span>Stored Partitions ({partitions.length})</span>
              <button
                type="button"
                onClick={fetchPartitions}
                disabled={loadingPartitions}
                className="text-accent hover:underline flex items-center space-x-1 cursor-pointer"
              >
                <RefreshCw className={`w-3 h-3 ${loadingPartitions ? "animate-spin" : ""}`} />
                <span>Refresh Partitions</span>
              </button>
            </div>

            <div className="max-h-48 overflow-y-auto border border-surface-border rounded-lg bg-surface divide-y divide-surface-border">
              {partitions.length === 0 ? (
                <div className="p-4 text-center text-slate-400 text-xs font-sans">
                  No partitions found or still retrieving.
                </div>
              ) : (
                partitions.slice(0, 50).map((p) => (
                  <div
                    key={p.relative_path}
                    onClick={() => setSelectedPartition(p.relative_path)}
                    className={`p-2.5 flex items-center justify-between text-xs transition cursor-pointer ${
                      selectedPartition === p.relative_path
                        ? "bg-accent-dim/40 border-l-2 border-accent"
                        : "hover:bg-slate-50"
                    }`}
                  >
                    <div className="flex items-center space-x-2">
                      <span className="px-1.5 py-0.5 rounded text-[10px] font-bold uppercase bg-slate-100 text-slate-700">
                        {p.venue}
                      </span>
                      <span className="font-bold text-slate-800">{p.symbol}</span>
                      <span className="text-slate-400">
                        {p.date} Hour: {p.hour}
                      </span>
                    </div>
                    <div className="flex items-center space-x-3">
                      <span className="text-slate-500 font-mono">{formatBytes(p.file_size_bytes)}</span>
                      <span className="text-[10px] px-1.5 py-0.5 bg-slate-100 rounded text-slate-600">
                        .jsonl.zst
                      </span>
                    </div>
                  </div>
                ))
              )}
            </div>
          </div>
        </div>
      )}

      {/* Preprocessing Pipelines & Custom Script Studio */}
      {isConnected && (
        <div className="p-4 bg-surface border border-surface-border rounded-xl shadow-2xs space-y-3">
          <div className="flex items-center justify-between border-b border-surface-border pb-3">
            <div>
              <h4 className="font-bold text-slate-900 flex items-center space-x-2">
                <Code className="w-4 h-4 text-accent-strong" />
                <span>Data Preprocessing &amp; Custom Scripts</span>
              </h4>
              <p className="text-xs text-slate-500 font-sans mt-0.5">
                Run prebuilt tick-to-bar aggregators or execute custom Python data cleaning scripts directly on the VPS.
              </p>
            </div>

            <div className="flex items-center space-x-1 p-0.5 bg-slate-100 rounded-lg border border-surface-border">
              <button
                type="button"
                onClick={() => setActivePipelineTab("prebuilt")}
                className={`px-3 py-1 rounded-md text-xs font-bold transition cursor-pointer ${
                  activePipelineTab === "prebuilt"
                    ? "bg-surface text-accent-strong shadow-2xs"
                    : "text-slate-600 hover:text-slate-900"
                }`}
              >
                Prebuilt Resampling
              </button>
              <button
                type="button"
                onClick={() => setActivePipelineTab("custom")}
                className={`px-3 py-1 rounded-md text-xs font-bold transition cursor-pointer ${
                  activePipelineTab === "custom"
                    ? "bg-surface text-accent-strong shadow-2xs"
                    : "text-slate-600 hover:text-slate-900"
                }`}
              >
                Custom Python Studio
              </button>
            </div>
          </div>

          {/* Prebuilt Resampling Tab */}
          {activePipelineTab === "prebuilt" && (
            <div className="space-y-3 pt-1">
              <div className="grid grid-cols-1 sm:grid-cols-3 gap-3">
                <div className="space-y-1">
                  <label className="text-slate-700 font-bold text-xs">Target Partition</label>
                  <input
                    type="text"
                    value={selectedPartition}
                    onChange={(e) => setSelectedPartition(e.target.value)}
                    placeholder="e.g. raw/binance/BTC-USDT-perp/..."
                    className="w-full px-3 py-1.5 bg-surface-raised border border-surface-border rounded-lg text-slate-900 text-xs"
                  />
                </div>

                <div className="space-y-1">
                  <label className="text-slate-700 font-bold text-xs">Bar Resample Interval</label>
                  <select
                    value={resampleInterval}
                    onChange={(e) => setResampleInterval(e.target.value)}
                    className="w-full px-3 py-1.5 bg-surface-raised border border-surface-border rounded-lg text-slate-900 text-xs"
                  >
                    <option value="1s">1 Second</option>
                    <option value="5s">5 Seconds</option>
                    <option value="1m">1 Minute</option>
                    <option value="5m">5 Minutes</option>
                    <option value="1h">1 Hour</option>
                  </select>
                </div>

                <div className="space-y-1">
                  <label className="text-slate-700 font-bold text-xs">Export Format</label>
                  <select
                    value={resampleFormat}
                    onChange={(e) => setResampleFormat(e.target.value)}
                    className="w-full px-3 py-1.5 bg-surface-raised border border-surface-border rounded-lg text-slate-900 text-xs"
                  >
                    <option value="csv">CSV (Comma-Separated)</option>
                    <option value="jsonl">JSON Lines</option>
                  </select>
                </div>
              </div>

              <div className="flex justify-end pt-1">
                <button
                  type="button"
                  onClick={handleRunPipeline}
                  disabled={isProcessing || !selectedPartition}
                  className="px-4 py-2 bg-accent hover:bg-accent-strong disabled:opacity-50 text-white font-bold rounded-lg transition shadow-2xs flex items-center space-x-1.5 cursor-pointer text-xs"
                >
                  <Play className={`w-3.5 h-3.5 ${isProcessing ? "animate-spin" : ""}`} />
                  <span>{isProcessing ? "Processing on VPS..." : "Execute Resampling Pipeline"}</span>
                </button>
              </div>

              {processResult && (
                <div className="p-3 bg-slate-900 text-emerald-400 rounded-lg text-xs font-mono overflow-x-auto">
                  <pre>{processResult}</pre>
                </div>
              )}
            </div>
          )}

          {/* Custom Python Script Studio */}
          {activePipelineTab === "custom" && (
            <div className="space-y-3 pt-1">
              <div className="flex items-center justify-between">
                <span className="text-slate-700 font-bold text-xs flex items-center space-x-1.5">
                  <Terminal className="w-3.5 h-3.5 text-slate-500" />
                  <span>Python 3 Script Editor (Runs in isolated subprocess on VPS)</span>
                </span>
                <button
                  type="button"
                  onClick={() =>
                    setCustomScript(
                      `#!/usr/bin/env python3\nimport sys, json, argparse\n\nparser = argparse.ArgumentParser()\nparser.add_argument("--input", required=True)\nparser.add_argument("--output", required=True)\nargs = parser.parse_args()\n\nprint(f"[Custom Script] Reading {args.input}...")\n# Custom feature calculation, CVD, or cleaning\n`
                    )
                  }
                  className="text-xs text-accent hover:underline cursor-pointer"
                >
                  Load Template
                </button>
              </div>

              <textarea
                value={customScript}
                onChange={(e) => setCustomScript(e.target.value)}
                rows={8}
                className="w-full p-3 bg-slate-950 text-slate-200 border border-slate-800 rounded-lg font-mono text-xs focus:outline-none focus:ring-1 focus:ring-accent leading-relaxed"
                spellCheck={false}
              />

              <div className="flex items-center justify-between pt-1">
                <span className="text-slate-500 text-[11px]">
                  Passes <code className="text-slate-700 font-bold">--input</code> and <code className="text-slate-700 font-bold">--output</code> arguments to your script.
                </span>

                <button
                  type="button"
                  onClick={handleRunCustomScript}
                  disabled={isProcessing || !customScript.trim()}
                  className="px-4 py-2 bg-accent hover:bg-accent-strong disabled:opacity-50 text-white font-bold rounded-lg transition shadow-2xs flex items-center space-x-1.5 cursor-pointer text-xs"
                >
                  <Play className={`w-3.5 h-3.5 ${isProcessing ? "animate-spin" : ""}`} />
                  <span>{isProcessing ? "Executing on VPS..." : "Run Script on VPS"}</span>
                </button>
              </div>

              {customOutput && (
                <div className="p-3 bg-slate-900 border border-slate-800 rounded-lg space-y-2 text-xs font-mono">
                  <div className="flex items-center justify-between">
                    <span className={`font-bold ${customOutput.success ? "text-emerald-400" : "text-loss"}`}>
                      {customOutput.success ? "Execution Succeeded" : "Execution Failed"}
                    </span>
                    <span className="text-slate-400 text-[11px]">{customOutput.duration_ms} ms</span>
                  </div>
                  {customOutput.stdout && (
                    <div>
                      <div className="text-slate-400 text-[10px] uppercase font-bold">Standard Output:</div>
                      <pre className="text-slate-200 text-xs overflow-x-auto whitespace-pre-wrap">{customOutput.stdout}</pre>
                    </div>
                  )}
                  {customOutput.stderr && (
                    <div>
                      <div className="text-loss text-[10px] uppercase font-bold">Standard Error:</div>
                      <pre className="text-loss text-xs overflow-x-auto whitespace-pre-wrap">{customOutput.stderr}</pre>
                    </div>
                  )}
                </div>
              )}
            </div>
          )}
        </div>
      )}
    </div>
  );
};
