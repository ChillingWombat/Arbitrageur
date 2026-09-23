import React, { useState, useEffect } from "react";
import { createPortal } from "react-dom";
import {
  AlertCircle,
  CheckCircle2,
  Key,
  Lock,
  Unlock,
  Sliders,
  ExternalLink,
  Globe,
  AlertTriangle,
  Scale,
  RefreshCw,
  Info,
  Trash2,
  ShieldAlert,
  Star,
} from "lucide-react";
import { api } from "../services/api";
import {
  normalizeRetentionDays,
  normalizeRetentionGb,
  retentionDaysToInput,
  retentionGbToInput,
} from "../services/retention";
import type { EngineSettingsDto, SystemStatusDto, VenueStatusDto, VaultStatus, VenueBalanceInfo, AccountDto, VenueLaneStatusDto, VenueCapabilityDto, PassiveUnwindDto, StorageStatsDto } from "../types";
import { DEFAULT_PASSIVE_UNWIND } from "../types";
import { VaultUnlockModal } from "./VaultUnlockModal";
import { InfoTooltip } from "./InfoTooltip";
import { SaveButton, useSaveButton } from "./SaveButton";
import { ArbitrageurLogo } from "./ArbitrageurLogo";
import { VenueLogo } from "./VenueLogo";
import { VenueAccountsSection } from "./ExchangeAccounts";


export type SettingsTab = "execution" | "safety" | "exchanges" | "network" | "about";
export type ExchangeId =
  | "arcus"
  | "aster"
  | "binance"
  | "coinbase"
  | "decibel"
  | "hyperliquid"
  | "ibkr"
  | "ig"
  | "kalshi"
  | "kraken"
  | "lighter"
  | "okx"
  | "polymarket"
  | "popdex"
  | "robinhood"
  | "vanta"
  // Long-tail perp venues (shared DexPerpAdapter; account-enabled only —
  // no engine-settings flags). Live-trading subset: aevo, derive, grvt,
  // ondoperps, pacifica, standx — the rest are market-data only.
  | "aevo"
  | "alphax"
  | "antarctic"
  | "apex"
  | "astros"
  | "bulk"
  | "derive"
  | "dydx"
  | "edgex"
  | "evedex"
  | "extended"
  | "gatedex"
  | "gmx"
  | "grvt"
  | "helix"
  | "iotrader"
  | "nado"
  | "niza"
  | "ondoperps"
  | "ostium"
  | "pacifica"
  | "paradex"
  | "qfex"
  | "sodex"
  | "standx"
  | "vest"
  // EVM spot AMMs (shared EvmAmmAdapter; account-enabled only). All are
  // live-capable via local-wallet signing; the account's optional baseUrl
  // field is the JSON-RPC endpoint override.
  | "uniswap"
  | "uniswap_v2"
  | "pancakeswap"
  | "pancakeswap_v3"
  | "sushiswap"
  | "quickswap"
  | "camelot"
  | "aerodrome"
  | "velodrome"
  | "ramses"
  | "pharaoh";

export interface ExchangeMetadata {
  id: ExchangeId;
  name: string;
  type: string;
  feeBps: number;
  /** Sidebar grouping — EVM AMMs cluster by chain (they share the chain's
   *  RPC endpoint; the account's baseUrl override is per-chain). */
  group: string;
}

/** Sidebar section order — primary venues first, then TradFi, the long-tail
 *  perp list, then EVM AMMs grouped by chain (same-chain venues share the
 *  chain RPC endpoint configured via the account's baseUrl). */
export const EXCHANGE_GROUP_ORDER = [
  "Primary",
  "TradFi",
  "Perp DEX — Long Tail",
  "EVM AMM — Ethereum",
  "EVM AMM — BSC",
  "EVM AMM — Arbitrum",
  "EVM AMM — Base",
  "EVM AMM — Optimism",
  "EVM AMM — Polygon",
  "EVM AMM — Avalanche",
];

/** Group a venue list into ordered sections for the sidebar. Within a group
 *  venues stay alphabetical. Unknown groups append last under their name. */
export function groupExchangesBySection(
  list: ExchangeMetadata[]
): { group: string; venues: ExchangeMetadata[] }[] {
  const byGroup = new Map<string, ExchangeMetadata[]>();
  for (const ex of list) {
    const g = ex.group;
    if (!byGroup.has(g)) byGroup.set(g, []);
    byGroup.get(g)!.push(ex);
  }
  const ordered = EXCHANGE_GROUP_ORDER.filter((g) => byGroup.has(g)).map(
    (g) => ({ group: g, venues: byGroup.get(g)! })
  );
  for (const [g, venues] of byGroup) {
    if (!EXCHANGE_GROUP_ORDER.includes(g)) ordered.push({ group: g, venues });
  }
  return ordered;
}

export const EXCHANGES_ALPHABETICAL: ExchangeMetadata[] = [
  { id: "aerodrome", name: "Aerodrome", type: "Spot AMM — Base (Market Data)", feeBps: 30.0 , group: "EVM AMM — Base" },
  { id: "aevo", name: "Aevo", type: "Perp DEX (Live)", feeBps: 3.0 , group: "Perp DEX — Long Tail" },
  { id: "alphax", name: "AlphaX", type: "Perp DEX (Market Data)", feeBps: 3.0 , group: "Perp DEX — Long Tail" },
  { id: "antarctic", name: "Antarctic", type: "Perp DEX (Market Data)", feeBps: 3.0 , group: "Perp DEX — Long Tail" },
  { id: "apex", name: "ApeX Omni", type: "Perp DEX (Market Data)", feeBps: 2.0 , group: "Perp DEX — Long Tail" },
  { id: "arcus", name: "Arcus", type: "CLOB DEX", feeBps: 1.5 , group: "Primary" },
  { id: "aster", name: "Aster", type: "Multi-Chain Perp DEX", feeBps: 3.5 , group: "Primary" },
  { id: "astros", name: "Astros", type: "Perp DEX (Market Data)", feeBps: 3.0 , group: "Perp DEX — Long Tail" },
  { id: "binance", name: "Binance", type: "CEX Spot & Perp", feeBps: 7.5 , group: "Primary" },
  { id: "bulk", name: "BULK", type: "Perp DEX (Market Data)", feeBps: 3.0 , group: "Perp DEX — Long Tail" },
  { id: "camelot", name: "Camelot V2", type: "Spot AMM — Arbitrum (Market Data)", feeBps: 30.0 , group: "EVM AMM — Arbitrum" },
  { id: "coinbase", name: "Coinbase", type: "CEX Spot & Perp", feeBps: 8.0 , group: "Primary" },
  { id: "decibel", name: "Decibel", type: "Aptos Perp DEX", feeBps: 2.5 , group: "Primary" },
  { id: "derive", name: "Derive", type: "Perp DEX (Live)", feeBps: 3.0 , group: "Perp DEX — Long Tail" },
  { id: "dydx", name: "dYdX Chain", type: "Perp DEX (Market Data)", feeBps: 2.0 , group: "Perp DEX — Long Tail" },
  { id: "edgex", name: "edgeX", type: "Perp DEX (Market Data)", feeBps: 3.8 , group: "Perp DEX — Long Tail" },
  { id: "evedex", name: "EVEDEX", type: "Perp DEX (Market Data)", feeBps: 3.0 , group: "Perp DEX — Long Tail" },
  { id: "extended", name: "Extended", type: "Perp DEX (Market Data)", feeBps: 2.0 , group: "Perp DEX — Long Tail" },
  { id: "gatedex", name: "Gate DEX", type: "Perp DEX (Market Data)", feeBps: 3.0 , group: "Perp DEX — Long Tail" },
  { id: "gmx", name: "GMX V2", type: "Perp AMM (Market Data)", feeBps: 5.0 , group: "Perp DEX — Long Tail" },
  { id: "grvt", name: "GRVT", type: "Perp DEX (Live)", feeBps: 5.0 , group: "Perp DEX — Long Tail" },
  { id: "helix", name: "Helix", type: "Perp DEX (Market Data)", feeBps: 0.01 , group: "Perp DEX — Long Tail" },
  { id: "hyperliquid", name: "Hyperliquid", type: "L1 Perp DEX", feeBps: 2.0 , group: "Primary" },
  { id: "ibkr", name: "Interactive Brokers", type: "TradFi Broker", feeBps: 1.0 , group: "TradFi" },
  { id: "ig", name: "IG Trading", type: "TradFi Broker", feeBps: 5.0 , group: "TradFi" },
  { id: "iotrader", name: "IO Trader", type: "Perp DEX (Market Data)", feeBps: 3.0 , group: "Perp DEX — Long Tail" },
  { id: "kalshi", name: "Kalshi", type: "CFTC Regulated", feeBps: 10.0 , group: "Primary" },
  { id: "kraken", name: "Kraken", type: "CEX Spot & Perp", feeBps: 16.0 , group: "Primary" },
  { id: "lighter", name: "Lighter", type: "zkDEX Rollup", feeBps: 1.5 , group: "Primary" },
  { id: "nado", name: "Nado", type: "Perp DEX (Market Data)", feeBps: 3.0 , group: "Perp DEX — Long Tail" },
  { id: "niza", name: "Niza", type: "Perp DEX (Market Data)", feeBps: 3.0 , group: "Perp DEX — Long Tail" },
  { id: "okx", name: "OKX", type: "CEX Spot & Perp", feeBps: 8.0 , group: "Primary" },
  { id: "ondoperps", name: "Ondo Perps", type: "Perp DEX (Live)", feeBps: 2.5 , group: "Perp DEX — Long Tail" },
  { id: "ostium", name: "Ostium", type: "Perp DEX (Market Data)", feeBps: 3.0 , group: "Perp DEX — Long Tail" },
  { id: "pacifica", name: "Pacifica", type: "Perp DEX (Live)", feeBps: 2.0 , group: "Perp DEX — Long Tail" },
  { id: "pancakeswap", name: "PancakeSwap V2", type: "Spot AMM — BSC (Live)", feeBps: 25.0 , group: "EVM AMM — BSC" },
  { id: "pancakeswap_v3", name: "PancakeSwap V3", type: "Spot AMM — BSC (Live)", feeBps: 25.0 , group: "EVM AMM — BSC" },
  { id: "paradex", name: "Paradex", type: "Perp DEX (Market Data)", feeBps: 2.0 , group: "Perp DEX — Long Tail" },
  { id: "pharaoh", name: "Pharaoh", type: "Spot AMM — Avalanche (Market Data)", feeBps: 30.0 , group: "EVM AMM — Avalanche" },
  { id: "polymarket", name: "Polymarket", type: "CTF Binary Market", feeBps: 0.0 , group: "Primary" },
  { id: "popdex", name: "PopDEX", type: "EVM Perp DEX", feeBps: 10.0 , group: "Primary" },
  { id: "qfex", name: "QFEX", type: "Perp DEX (Market Data)", feeBps: 4.0 , group: "Perp DEX — Long Tail" },
  { id: "quickswap", name: "QuickSwap", type: "Spot AMM — Polygon (Live)", feeBps: 30.0 , group: "EVM AMM — Polygon" },
  { id: "ramses", name: "Ramses", type: "Spot AMM — Arbitrum (Market Data)", feeBps: 30.0 , group: "EVM AMM — Arbitrum" },
  { id: "robinhood", name: "Robinhood Lighter", type: "EVM DEX Aggregator", feeBps: 2.5 , group: "Primary" },
  { id: "sodex", name: "SoDEX", type: "Perp DEX (Market Data)", feeBps: 3.0 , group: "Perp DEX — Long Tail" },
  { id: "standx", name: "StandX", type: "Perp DEX (Live)", feeBps: 4.0 , group: "Perp DEX — Long Tail" },
  { id: "sushiswap", name: "SushiSwap", type: "Spot AMM — Ethereum (Live)", feeBps: 30.0 , group: "EVM AMM — Ethereum" },
  { id: "uniswap", name: "Uniswap V3", type: "Spot AMM — Ethereum (Live)", feeBps: 5.0 , group: "EVM AMM — Ethereum" },
  { id: "uniswap_v2", name: "Uniswap V2", type: "Spot AMM — Ethereum (Live)", feeBps: 30.0 , group: "EVM AMM — Ethereum" },
  { id: "vanta", name: "Vanta", type: "Omnichain Perps", feeBps: 3.0 , group: "Primary" },
  { id: "velodrome", name: "Velodrome", type: "Spot AMM — Optimism (Market Data)", feeBps: 30.0 , group: "EVM AMM — Optimism" },
  { id: "vest", name: "Vest", type: "Perp DEX (Market Data)", feeBps: 3.0 , group: "Perp DEX — Long Tail" },
];

interface EngineSettingsProps {
  systemStatus?: SystemStatusDto | null;
  onClose?: () => void;
  onSettingsSaved?: (newSettings: EngineSettingsDto) => void;
  /** Optional header portal target (SettingsModal's title bar) — the vault
   *  lock/unlock button renders there instead of the tab action row. */
  headerActionsEl?: HTMLDivElement | null;
}

// Custom equality for React.memo: the component only reads
// systemStatus.active_venues, so skip re-rendering when the 3s status poll
// produces identical venue data (the common case — latency probes update
// only every 30s). Without this the whole settings tree re-renders on
// every poll and scrolling stutters.
function areEngineSettingsPropsEqual(p: EngineSettingsProps, n: EngineSettingsProps): boolean {
  if (p.onClose !== n.onClose || p.onSettingsSaved !== n.onSettingsSaved) return false;
  if (p.headerActionsEl !== n.headerActionsEl) return false;
  if (p.systemStatus === n.systemStatus) return true;
  return (
    JSON.stringify(p.systemStatus?.active_venues ?? null) ===
    JSON.stringify(n.systemStatus?.active_venues ?? null)
  );
}

function formatRetainedSpan(oldestMs: number | null, newestMs: number | null): string {
  if (oldestMs == null || newestMs == null || newestMs <= oldestMs) return "—";
  const days = (newestMs - oldestMs) / 86_400_000;
  if (days >= 1) return `${days.toFixed(1)} days`;
  return `${((newestMs - oldestMs) / 3_600_000).toFixed(1)} hours`;
}

function formatStorageBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return "—";
  if (bytes >= 1 << 30) return `${(bytes / (1 << 30)).toFixed(2)} GB`;
  if (bytes >= 1 << 20) return `${(bytes / (1 << 20)).toFixed(1)} MB`;
  return `${Math.max(1, Math.round(bytes / 1024))} KB`;
}

export const EngineSettings = React.memo<EngineSettingsProps>(({ systemStatus, onClose, onSettingsSaved, headerActionsEl }) => {
  const [liveVenues, setLiveVenues] = useState<VenueStatusDto[]>([]);
  const [pinging, setPinging] = useState(false);

  // Remote HSM
  const [hsmUrl, setHsmUrl] = useState("http://127.0.0.1:8787/sign");
  const [hmacToken, setHmacToken] = useState("");

  // Guardrails & Execution Modes (slippage is per-strategy now: each bot
  // configurator carries its own max_slippage_bps gate)
  const [minDepthRatio, setMinDepthRatio] = useState(3.0);
  // Global Auto Position Exit policy — strategies on "inherit" use this;
  // the Portfolio Monitor's Auto Limit exit runs it too.
  const [passiveUnwind, setPassiveUnwind] = useState<PassiveUnwindDto>({ ...DEFAULT_PASSIVE_UNWIND });
  // Auto-exit hold deadline (minutes in the UI; seconds on the wire).
  const [autoExitMaxHoldMin, setAutoExitMaxHoldMin] = useState(1440);
  // Historical retention: empty string = unlimited (None on the backend)
  const [maxHistoryDays, setMaxHistoryDays] = useState("");
  const [maxStorageGb, setMaxStorageGb] = useState("");
  const [storageStats, setStorageStats] = useState<StorageStatsDto | null>(null);
  const [tradingMode, setTradingMode] = useState<"testnet" | "live">("testnet");

  // Safety: overall circuit-breaker + safety-net limits
  const [cbMaxDdUsd, setCbMaxDdUsd] = useState(25000);
  const [cbMaxDdPct, setCbMaxDdPct] = useState(5);
  const [cbMaxErrors, setCbMaxErrors] = useState(5);
  const [cbMaxSpreadBps, setCbMaxSpreadBps] = useState(250);
  const [deltaMaxUnhedged, setDeltaMaxUnhedged] = useState(2500);
  const [marginWarnPct, setMarginWarnPct] = useState(70);
  const [marginCritPct, setMarginCritPct] = useState(85);
  const [marginMinLiqPct, setMarginMinLiqPct] = useState(15);
  const [maxLeverage, setMaxLeverage] = useState(10);
  const [liveEntryEnabled, setLiveEntryEnabled] = useState(false);
  const [liveEntryMaxNotional, setLiveEntryMaxNotional] = useState(0);
  // Canary run state (Safety → live entries)
  const [canarySymbol, setCanarySymbol] = useState("ETH-USDC");
  const [canaryBuyVenue, setCanaryBuyVenue] = useState("hyperliquid");
  const [canarySellVenue, setCanarySellVenue] = useState("lighter");
  const [canaryQty, setCanaryQty] = useState(0.005);
  const [canaryLossBps, setCanaryLossBps] = useState(25);
  const [canaryRunning, setCanaryRunning] = useState(false);
  const [canaryResult, setCanaryResult] = useState<string | null>(null);
  const [staleWarnSecs, setStaleWarnSecs] = useState(300);
  // Networking & VPS

  // Exchange Enablement Toggles
  const [hlEnabled, setHlEnabled] = useState(false);
  const [lighterEnabled, setLighterEnabled] = useState(false);
  const [vantaEnabled, setVantaEnabled] = useState(false);
  const [robinhoodEnabled, setRobinhoodEnabled] = useState(false);
  const [krakenEnabled, setKrakenEnabled] = useState(false);
  const [krakenSpotEnabled, setKrakenSpotEnabled] = useState(false);
  const [krakenFuturesEnabled, setKrakenFuturesEnabled] = useState(false);
  const [polymarketEnabled, setPolymarketEnabled] = useState(false);
  const [kalshiEnabled, setKalshiEnabled] = useState(false);

  // Hyperliquid
  const [hyperliquidWallet, setHyperliquidWallet] = useState("");
  const [hyperliquidPrivateKey, setHyperliquidPrivateKey] = useState("");
  const [hyperliquidUrl, setHyperliquidUrl] = useState("https://api.hyperliquid.xyz");
  const [hyperliquidAccountIndex, setHyperliquidAccountIndex] = useState<string>("");
  const [hyperliquidSubaccountIndex, setHyperliquidSubaccountIndex] = useState<string>("");

  // Lighter
  const [lighterAccountIndex, setLighterAccountIndex] = useState("0");
  const [lighterApiKey, setLighterApiKey] = useState("");
  const [lighterUrl, setLighterUrl] = useState("https://mainnet.zklighter.elliot.ai");
  const [lighterSubaccountIndex, setLighterSubaccountIndex] = useState<string>("");

  // Vanta (Orderly)
  const [vantaAccountId, setVantaAccountId] = useState("");
  const [vantaOrderlyKey, setVantaOrderlyKey] = useState("");
  const [vantaOrderlySecret, setVantaOrderlySecret] = useState("");
  const [vantaUrl, setVantaUrl] = useState("https://api.orderly.org");
  const [vantaSubaccountIndex, setVantaSubaccountIndex] = useState<string>("");

  // Robinhood EVM
  const [robinhoodWallet, setRobinhoodWallet] = useState("");
  const [robinhoodPrivateKey, setRobinhoodPrivateKey] = useState("");
  const [robinhoodRouter, setRobinhoodRouter] = useState("0x89e5DB8B5aA49aA85AC63f691524311AEB649eba");
  const [robinhoodSubaccountIndex, setRobinhoodSubaccountIndex] = useState<string>("");

  // Arcus CLOB DEX
  const [arcusEnabled, setArcusEnabled] = useState(false);
  const [asterEnabled, setAsterEnabled] = useState(false);
  const [popdexEnabled, setPopdexEnabled] = useState(false);
  const [popdexNetwork, setPopdexNetwork] = useState("mainnet");
  const [binanceEnabled, setBinanceEnabled] = useState(false);
  const [coinbaseEnabled, setCoinbaseEnabled] = useState(false);
  const [decibelEnabled, setDecibelEnabled] = useState(false);
  const [ibkrEnabled, setIbkrEnabled] = useState(false);
  const [igEnabled, setIgEnabled] = useState(false);
  const [okxEnabled, setOkxEnabled] = useState(false);
  const [arcusApiKey, setArcusApiKey] = useState("");
  const [arcusSecret, setArcusSecret] = useState("");
  const [arcusUrl, setArcusUrl] = useState("https://api.arcus.exchange");

  // Kraken (Spot & Futures)
  const [krakenSpotApiKey, setKrakenSpotApiKey] = useState("");
  const [krakenSpotSecret, setKrakenSpotSecret] = useState("");
  const [krakenSpotUrl, setKrakenSpotUrl] = useState("https://api.kraken.com");
  const [krakenFuturesApiKey, setKrakenFuturesApiKey] = useState("");
  const [krakenFuturesSecret, setKrakenFuturesSecret] = useState("");
  const [krakenFuturesUrl, setKrakenFuturesUrl] = useState("https://futures.kraken.com");

  // Polymarket
  const [polymarketApiKey, setPolymarketApiKey] = useState("");
  const [polymarketSecret, setPolymarketSecret] = useState("");
  const [polymarketPassphrase, setPolymarketPassphrase] = useState("");
  const [polymarketUrl, setPolymarketUrl] = useState("https://clob.polymarket.com");

  // Kalshi
  const [kalshiApiKeyId, setKalshiApiKeyId] = useState("");
  const [kalshiPrivateKey, setKalshiPrivateKey] = useState("");
  const [kalshiUrl, setKalshiUrl] = useState("https://trading-api.kalshi.com");

  // Referrals, About & Links
  const [sponsorLink, setSponsorLink] = useState("https://buymeacoffee.com/dav1d1010");
  const [version, setVersion] = useState("v1.2.0");
  const [gitRepo, setGitRepo] = useState("https://github.com/Dav1D1010/Arbitrageur");

  // UI state
  const [activeTab, setActiveTab] = useState<SettingsTab>("execution");
  // Risk Control sub-category anchors (scroll-to-section pills).
  const [selectedExchange, setSelectedExchange] = useState<ExchangeId>("arcus");
  // Favorite exchanges: a pure UI preference (localStorage, per machine) —
  // starring never touches strategy state. The selector filter below narrows
  // the tab list to favorites for operators who only use a few venues.
  const FAVORITE_KEY = "arbitrageur.favoriteExchanges.v1";
  const [favoriteExchanges, setFavoriteExchanges] = useState<string[]>(() => {
    try {
      const raw = localStorage.getItem(FAVORITE_KEY);
      const arr = raw ? JSON.parse(raw) : [];
      return Array.isArray(arr) ? arr.filter((v) => typeof v === "string") : [];
    } catch {
      return [];
    }
  });
  const [exchangeFilter, setExchangeFilter] = useState<"all" | "favorites">("all");
  const toggleFavorite = (id: string) =>
    setFavoriteExchanges((prev) => {
      const next = prev.includes(id) ? prev.filter((v) => v !== id) : [...prev, id];
      try {
        localStorage.setItem(FAVORITE_KEY, JSON.stringify(next));
      } catch {
        /* private-mode storage — favorites simply don't persist */
      }
      return next;
    });
  const visibleExchanges = EXCHANGES_ALPHABETICAL.filter(
    (ex) => exchangeFilter === "all" || favoriteExchanges.includes(ex.id),
  );
  // Keep the selection visible: if the active tab is filtered out, fall back
  // to the first visible exchange (or keep it when the filter empties).
  useEffect(() => {
    if (!visibleExchanges.some((ex) => ex.id === selectedExchange) && visibleExchanges.length > 0) {
      setSelectedExchange(visibleExchanges[0].id);
    }
  }, [exchangeFilter, favoriteExchanges]); // eslint-disable-line react-hooks/exhaustive-deps
  const [loading, setLoading] = useState(true);
  const saveBtn = useSaveButton();
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  // Vault state
  const [vaultStatus, setVaultStatus] = useState<VaultStatus | null>(null);
  const [showVaultModal, setShowVaultModal] = useState(false);
  const [configuredVenues, setConfiguredVenues] = useState<string[]>([]);

  // Venue execution capabilities & live trading risk gating
  const [venueCapabilities, setVenueCapabilities] = useState<VenueCapabilityDto[]>([]);
  const [showLiveRiskModal, setShowLiveRiskModal] = useState(false);
  const [liveRiskAgreed, setLiveRiskAgreed] = useState(false);
  const [liveRiskConfirmationInput, setLiveRiskConfirmationInput] = useState("");
  const [liveRiskAck, setLiveRiskAck] = useState(false);

  // Venue Balances
  const [venueBalances, setVenueBalances] = useState<VenueBalanceInfo[]>([]);
  const [venueLanes, setVenueLanes] = useState<VenueLaneStatusDto[]>([]);
  const [accountsEpoch, setAccountsEpoch] = useState(0);
  // All account profiles — drives sidebar enable/configured badges so they
  // reflect account-level state, not just the legacy venue flags.
  const [accounts, setAccounts] = useState<AccountDto[]>([]);

  useEffect(() => {
    api.listAccounts().then(setAccounts).catch(() => setAccounts([]));
  }, [accountsEpoch]);


  // Update check
  const [checkingUpdate, setCheckingUpdate] = useState(false);
  const [updateMsg, setUpdateMsg] = useState<string | null>(null);

  const venueFlagEnabled = (id: ExchangeId): boolean => {
    switch (id) {
      case "arcus": return arcusEnabled;
      case "aster": return asterEnabled;
      case "popdex": return popdexEnabled;
      case "binance": return binanceEnabled;
      case "coinbase": return coinbaseEnabled;
      case "decibel": return decibelEnabled;
      case "hyperliquid": return hlEnabled;
      case "ibkr": return ibkrEnabled;
      case "ig": return igEnabled;
      case "kalshi": return kalshiEnabled;
      case "kraken": return krakenEnabled;
      case "lighter": return lighterEnabled;
      case "okx": return okxEnabled;
      case "polymarket": return polymarketEnabled;
      case "robinhood": return robinhoodEnabled;
      case "vanta": return vantaEnabled;
      default: return false;
    }
  };

  const setVenueFlagEnabled = (id: ExchangeId, enabled: boolean) => {
    switch (id) {
      case "arcus": setArcusEnabled(enabled); break;
      case "aster": setAsterEnabled(enabled); break;
      case "popdex": setPopdexEnabled(enabled); break;
      case "binance": setBinanceEnabled(enabled); break;
      case "coinbase": setCoinbaseEnabled(enabled); break;
      case "decibel": setDecibelEnabled(enabled); break;
      case "hyperliquid": setHlEnabled(enabled); break;
      case "ibkr": setIbkrEnabled(enabled); break;
      case "ig": setIgEnabled(enabled); break;
      case "kalshi": setKalshiEnabled(enabled); break;
      case "kraken": setKrakenEnabled(enabled); break;
      case "lighter": setLighterEnabled(enabled); break;
      case "okx": setOkxEnabled(enabled); break;
      case "polymarket": setPolymarketEnabled(enabled); break;
      case "robinhood": setRobinhoodEnabled(enabled); break;
      case "vanta": setVantaEnabled(enabled); break;
    }
  };

  // Kraken's two account zones are separate venues upstream of the sidebar id.
  const accountsForVenue = (id: string): AccountDto[] =>
    id === "kraken"
      ? accounts.filter((a) => a.venue === "kraken" || a.venue === "kraken_spot" || a.venue === "kraken_futures")
      : accounts.filter((a) => a.venue === id);

  // Same admission rule as the backend: venue flag OR any enabled account.
  const isExchangeEnabled = (id: ExchangeId): boolean =>
    venueFlagEnabled(id) || accountsForVenue(id).some((a) => a.enabled);

  const hasConfiguredAccount = (id: string): boolean =>
    accountsForVenue(id).some((a) => a.credentialsConfigured === true);

  const getVenueBalance = (id: string): VenueBalanceInfo | null => {
    return (
      venueBalances.find((b) => b.venue.toLowerCase() === id.toLowerCase()) ?? null
    );
  };

  // Spread-monitor rate lane for this venue (read budget usage), when streaming.
  const getVenueLane = (id: string): VenueLaneStatusDto | null => {
    return (
      venueLanes.find((l) => l.venue.toLowerCase() === id.toLowerCase()) ?? null
    );
  };

  // Measured round-trip to the venue API, from the live system status poll.
  const getVenueLatency = (id: string): number | null => {
    return (
      liveVenues.find((v) => v.id.toLowerCase() === id.toLowerCase())?.latency_ms ?? null
    );
  };

  // Backend admission status for this venue — OFFLINE (not enabled),
  // READ_ONLY (admitted, streaming market data) or UNSUPPORTED (enabled but
  // no market-data adapter; contributes nothing to ticker availability).
  const getVenueStatus = (id: string): string | null => {
    return (
      liveVenues.find((v) => v.id.toLowerCase() === id.toLowerCase())?.status ?? null
    );
  };


  const latencyClass = (ms: number): string =>
    ms < 100 ? "text-profit font-semibold" : ms < 300 ? "text-warn-strong font-semibold" : "text-loss font-semibold";

  const loadVaultStatus = async () => {
    try {
      const status = await api.getVaultStatus();
      setVaultStatus(status);
    } catch (err) {
      console.warn("Could not fetch vault status:", err);
    }
  };

  const loadVenueBalances = async () => {
    try {
      const data = await api.getVenueBalances();
      setVenueBalances(data);
    } catch (err) {
      console.error("Failed to load balances:", err);
    }
  };

  useEffect(() => {
    const loadSettings = async () => {
      try {
        const s: EngineSettingsDto = await api.getEngineSettings();
        setConfiguredVenues(s.configured_venues || []);
        api.getVenueExecutionCapabilities().then(setVenueCapabilities).catch(console.error);
        setHsmUrl(s.hsm_url || "http://127.0.0.1:8787/sign");
        setHmacToken(s.hmac_token || "");
        setMinDepthRatio(s.min_depth_ratio ?? 3.0);
        setPassiveUnwind({
          enabled: s.passive_unwind?.enabled === true,
          spreadPercentile: s.passive_unwind?.spreadPercentile ?? DEFAULT_PASSIVE_UNWIND.spreadPercentile,
          timeoutSecs: s.passive_unwind?.timeoutSecs ?? DEFAULT_PASSIVE_UNWIND.timeoutSecs,
          maxTries: s.passive_unwind?.maxTries ?? DEFAULT_PASSIVE_UNWIND.maxTries,
          maxSpreadBps: s.passive_unwind?.maxSpreadBps ?? DEFAULT_PASSIVE_UNWIND.maxSpreadBps,
          maxFastVolBps: s.passive_unwind?.maxFastVolBps ?? DEFAULT_PASSIVE_UNWIND.maxFastVolBps,
        });
        setAutoExitMaxHoldMin(Math.round((s.auto_exit_max_hold_secs ?? 86400) / 60));
        setMaxHistoryDays(retentionDaysToInput(s.spread_samples_max_days));
        setMaxStorageGb(retentionGbToInput(s.spread_samples_max_gb));


        setTradingMode(s.trading_mode === "live" ? "live" : "testnet");
        setCbMaxDdUsd(s.cb_max_daily_drawdown_usd ?? 25000);
        setCbMaxDdPct(s.cb_max_daily_drawdown_pct ?? 5);
        setCbMaxErrors(s.cb_max_consecutive_errors ?? 5);
        setCbMaxSpreadBps(s.cb_max_spread_divergence_bps ?? 250);
        setDeltaMaxUnhedged(s.delta_max_unhedged_usd_per_asset ?? 2500);
        setMarginWarnPct(s.margin_warning_usage_pct ?? 70);
        setMarginCritPct(s.margin_critical_usage_pct ?? 85);
        setMarginMinLiqPct(s.margin_min_liquidation_distance_pct ?? 15);
        setMaxLeverage(s.max_leverage ?? 10);
        setLiveEntryEnabled(s.live_entry_enabled ?? false);
        setLiveEntryMaxNotional(s.live_entry_max_notional_usd ?? 0);
        setStaleWarnSecs(s.balance_stale_warn_secs ?? 300);

        setHyperliquidWallet(s.hyperliquid_wallet || "");
        setHyperliquidPrivateKey(s.hyperliquid_private_key || "");
        setHyperliquidUrl(s.hyperliquid_url || "https://api.hyperliquid.xyz");
        setHyperliquidAccountIndex(s.hyperliquid_account_index?.toString() ?? "");
        setHyperliquidSubaccountIndex(s.hyperliquid_subaccount_index?.toString() ?? "");

        setLighterAccountIndex(s.lighter_account_index || "0");
        setLighterApiKey(s.lighter_api_key || "");
        setLighterUrl(s.lighter_url || "https://mainnet.zklighter.elliot.ai");
        setLighterSubaccountIndex(s.lighter_subaccount_index?.toString() ?? "");

        setVantaAccountId(s.vanta_account_id || "");
        setVantaOrderlyKey(s.vanta_orderly_key || "");
        setVantaOrderlySecret(s.vanta_orderly_secret || "");
        setVantaUrl(s.vanta_url || "https://api.orderly.org");
        setVantaSubaccountIndex(s.vanta_subaccount_index?.toString() ?? "");

        setRobinhoodWallet(s.robinhood_wallet || "");
        setRobinhoodPrivateKey(s.robinhood_private_key || "");
        setRobinhoodRouter(s.robinhood_router || "0x89e5DB8B5aA49aA85AC63f691524311AEB649eba");
        setRobinhoodSubaccountIndex(s.robinhood_subaccount_index?.toString() ?? "");

        setPolymarketApiKey(s.polymarket_api_key || "");
        setPolymarketSecret(s.polymarket_secret || "");
        setPolymarketPassphrase(s.polymarket_passphrase || "");
        setPolymarketUrl(s.polymarket_url || "https://clob.polymarket.com");

        setKalshiApiKeyId(s.kalshi_api_key_id || "");
        setKalshiPrivateKey(s.kalshi_private_key || "");
        setKalshiUrl(s.kalshi_url || "https://trading-api.kalshi.com");

        setHlEnabled(s.hyperliquid_enabled ?? false);
        setLighterEnabled(s.lighter_enabled ?? false);
        setVantaEnabled(s.vanta_enabled ?? false);
        setRobinhoodEnabled(s.robinhood_enabled ?? false);
        setArcusEnabled(s.arcus_enabled ?? false);
        setKrakenEnabled(s.kraken_enabled ?? false);
        setKrakenSpotEnabled(s.kraken_spot_enabled ?? false);
        setKrakenFuturesEnabled(s.kraken_futures_enabled ?? false);
        setPolymarketEnabled(s.polymarket_enabled ?? false);
        setKalshiEnabled(s.kalshi_enabled ?? false);
        setDecibelEnabled(s.decibel_enabled ?? false);
        setIbkrEnabled(s.ibkr_enabled ?? false);
        setIgEnabled(s.ig_enabled ?? false);
        setBinanceEnabled(s.binance_enabled ?? false);
        setOkxEnabled(s.okx_enabled ?? false);
        setAsterEnabled(s.aster_enabled ?? false);
        setPopdexEnabled(s.popdex_enabled ?? false);
        setPopdexNetwork((s as unknown as Record<string, unknown>).popdex_network === "testnet" ? "testnet" : "mainnet");
        setCoinbaseEnabled(s.coinbase_enabled ?? false);

        setArcusApiKey(s.arcus_api_key || "");
        setArcusSecret(s.arcus_secret || "");
        setArcusUrl(s.arcus_url || "https://api.arcus.exchange");

        setKrakenSpotApiKey(s.kraken_spot_api_key || "");
        setKrakenSpotSecret(s.kraken_spot_secret || "");
        setKrakenSpotUrl(s.kraken_spot_url || "https://api.kraken.com");
        setKrakenFuturesApiKey(s.kraken_futures_api_key || "");
        setKrakenFuturesSecret(s.kraken_futures_secret || "");
        setKrakenFuturesUrl(s.kraken_futures_url || "https://futures.kraken.com");

        setSponsorLink(s.sponsor_link || "https://buymeacoffee.com/dav1d1010");
        setVersion(s.version || "v1.2.0");
        setGitRepo(s.git_repo || "https://github.com/Dav1D1010/Arbitrageur");

        if (systemStatus?.active_venues && systemStatus.active_venues.length > 0) {
          setLiveVenues(systemStatus.active_venues);
        } else {
          api.getSystemStatus().then((sys) => {
            if (sys?.active_venues) setLiveVenues(sys.active_venues);
          }).catch(() => {});
        }
      } catch (err) {
        console.error("Failed to load engine settings:", err);
      } finally {
        setLoading(false);
      }
    };
    loadSettings();
    loadVaultStatus();
    loadVenueBalances();
  }, []);

  // Venue read-budget lanes stream from the spread monitor — poll lightly
  // while the settings page is mounted.
  useEffect(() => {
    let dead = false;
    const tick = async () => {
      try {
        const s = await api.getMonitorSnapshot();
        if (!dead && s) setVenueLanes(s.venues);
      } catch {
        /* monitor engine not running — lanes stay empty */
      }
    };
    tick();
    const id = setInterval(tick, 5000);
    return () => {
      dead = true;
      clearInterval(id);
    };
  }, []);

  // Retained-history footprint behind the retention inputs — one table
  // scan per tick, so poll slower than the venue lanes.
  useEffect(() => {
    let dead = false;
    const tick = async () => {
      try {
        const s = await api.getStorageStats();
        if (!dead) setStorageStats(s);
      } catch {
        /* store unavailable — stats stay empty */
      }
    };
    tick();
    const id = setInterval(tick, 15000);
    return () => {
      dead = true;
      clearInterval(id);
    };
  }, []);

  useEffect(() => {
    if (systemStatus?.active_venues && systemStatus.active_venues.length > 0) {
      setLiveVenues(systemStatus.active_venues);
    }
  }, [systemStatus]);

  // A venue only counts as "API connected" when enabled and backed by a
  // working adapter (READ_ONLY). OFFLINE/UNSUPPORTED venues provide no data.
  const hasApiConnectivity = liveVenues.some((v) => v.status === "READ_ONLY");

  useEffect(() => {
    api.getVenueExecutionCapabilities().then(setVenueCapabilities).catch(console.error);
  }, [vaultStatus]);

  const submissionReadyVenues = venueCapabilities.filter((c) => c.submissionReady);
  const canSwitchToLive = (vaultStatus?.is_unlocked ?? false) && submissionReadyVenues.length >= 2;

  const handleSave = async (e: React.FormEvent) => {
    e.preventDefault();
    setErrorMsg(null);
    await saveBtn.run(async () => {
      try {
        const payload: EngineSettingsDto = {
          hsm_url: hsmUrl,
          hmac_token: hmacToken,
          min_depth_ratio: minDepthRatio,
          engine_rpc_port: 8787, // reserved — no desktop listener consumes it
          engine_ws_port: 8789, // reserved — no desktop listener consumes it
          bind_address: "127.0.0.1", // reserved — no desktop listener consumes it
          trading_mode: tradingMode,
          fast_warmup_enabled: true, // reserved — warm-up reuse not implemented
          cb_max_daily_drawdown_usd: Number(cbMaxDdUsd) || 0,
          cb_max_daily_drawdown_pct: Number(cbMaxDdPct) || 0,
          cb_max_consecutive_errors: Math.trunc(Number(cbMaxErrors)) || 0,
          cb_max_spread_divergence_bps: Number(cbMaxSpreadBps) || 0,
          delta_max_unhedged_usd_per_asset: Number(deltaMaxUnhedged) || 0,
          margin_warning_usage_pct: Number(marginWarnPct) || 0,
          margin_critical_usage_pct: Number(marginCritPct) || 0,
          margin_min_liquidation_distance_pct: Number(marginMinLiqPct) || 0,
          max_leverage: Math.trunc(Number(maxLeverage)) || 0,
          live_entry_enabled: liveEntryEnabled,
          live_entry_max_notional_usd: Number(liveEntryMaxNotional) || 0,
          passive_unwind: {
            enabled: passiveUnwind.enabled,
            spreadPercentile: Number(passiveUnwind.spreadPercentile) || 0,
            timeoutSecs: Math.trunc(Number(passiveUnwind.timeoutSecs)) || 0,
            maxTries: Math.trunc(Number(passiveUnwind.maxTries)) || 0,
            maxSpreadBps: Number(passiveUnwind.maxSpreadBps) || 0,
            maxFastVolBps: Number(passiveUnwind.maxFastVolBps) || 0,
          },
          auto_exit_max_hold_secs: Math.trunc(Number(autoExitMaxHoldMin) * 60) || 0,
          balance_stale_warn_secs: Math.trunc(Number(staleWarnSecs)) || 0,
          spread_samples_max_days: normalizeRetentionDays(maxHistoryDays),
          spread_samples_max_gb: normalizeRetentionGb(maxStorageGb),
          configured_venues: configuredVenues,
          hyperliquid_enabled: hlEnabled,
          lighter_enabled: lighterEnabled,
          vanta_enabled: vantaEnabled,
          robinhood_enabled: robinhoodEnabled,
          arcus_enabled: arcusEnabled,
          kraken_enabled: krakenEnabled,
          kraken_spot_enabled: krakenSpotEnabled,
          kraken_futures_enabled: krakenFuturesEnabled,
          polymarket_enabled: polymarketEnabled,
          kalshi_enabled: kalshiEnabled,
          decibel_enabled: decibelEnabled,
          ibkr_enabled: ibkrEnabled,
          ig_enabled: igEnabled,
          binance_enabled: binanceEnabled,
          okx_enabled: okxEnabled,
          aster_enabled: asterEnabled,
          popdex_enabled: popdexEnabled,
          popdex_network: popdexNetwork,
          coinbase_enabled: coinbaseEnabled,
          arcus_api_key: arcusApiKey.trim(),
          arcus_secret: arcusSecret.trim(),
          arcus_url: arcusUrl.trim(),
          kraken_spot_api_key: krakenSpotApiKey.trim(),
          kraken_spot_secret: krakenSpotSecret.trim(),
          kraken_spot_url: krakenSpotUrl.trim(),
          kraken_futures_api_key: krakenFuturesApiKey.trim(),
          kraken_futures_secret: krakenFuturesSecret.trim(),
          kraken_futures_url: krakenFuturesUrl.trim(),
          hyperliquid_wallet: hyperliquidWallet.trim(),
          hyperliquid_private_key: hyperliquidPrivateKey.trim(),
          hyperliquid_url: hyperliquidUrl.trim(),
          hyperliquid_account_index: hyperliquidAccountIndex ? parseInt(hyperliquidAccountIndex, 10) : undefined,
          hyperliquid_subaccount_index: hyperliquidSubaccountIndex ? parseInt(hyperliquidSubaccountIndex, 10) : undefined,
          lighter_account_index: lighterAccountIndex.trim(),
          lighter_api_key: lighterApiKey.trim(),
          lighter_url: lighterUrl.trim(),
          lighter_subaccount_index: lighterSubaccountIndex ? parseInt(lighterSubaccountIndex, 10) : undefined,
          vanta_account_id: vantaAccountId.trim(),
          vanta_orderly_key: vantaOrderlyKey.trim(),
          vanta_orderly_secret: vantaOrderlySecret.trim(),
          vanta_url: vantaUrl.trim(),
          vanta_subaccount_index: vantaSubaccountIndex ? parseInt(vantaSubaccountIndex, 10) : undefined,
          robinhood_wallet: robinhoodWallet.trim(),
          robinhood_private_key: robinhoodPrivateKey.trim(),
          robinhood_rpc_url: "https://rpc.robinhood.network", // reserved — chain RPC unused
          robinhood_router: robinhoodRouter.trim(),
          robinhood_subaccount_index: robinhoodSubaccountIndex ? parseInt(robinhoodSubaccountIndex, 10) : undefined,
          polymarket_api_key: polymarketApiKey.trim(),
          polymarket_secret: polymarketSecret.trim(),
          polymarket_passphrase: polymarketPassphrase.trim(),
          polymarket_url: polymarketUrl.trim(),
          kalshi_api_key_id: kalshiApiKeyId.trim(),
          kalshi_private_key: kalshiPrivateKey.trim(),
          kalshi_url: kalshiUrl.trim(),
          referral_lighter: "https://app.lighter.xyz/?referral=SOLO&source=none",
          referral_robinhood: "https://robinhoodchain.lighter.xyz/?referral=SOLO&source=none",
          sponsor_link: sponsorLink,
          version: version,
          git_repo: gitRepo,
        };

        await api.saveEngineSettings(payload);

        // Ensure live mode state reflects selected mode
        await api.setTradingEnvironment(tradingMode);

        const sanitized = await api.getEngineSettings();
        setConfiguredVenues(sanitized.configured_venues || []);
        clearCredentialInputs();

        // Refresh balances for the newly active trading mode
        await loadVenueBalances();

        onSettingsSaved?.(sanitized);
      } catch (err: unknown) {
        const msg = err instanceof Error ? err.message : String(err);
        setErrorMsg(msg);
        throw err; // rethrow so the save button lands on "Failed!"
      }
    });
  };

  const clearCredentialInputs = () => {
    setHmacToken("");
    setHyperliquidWallet("");
    setHyperliquidPrivateKey("");
    setHyperliquidAccountIndex("");
    setHyperliquidSubaccountIndex("");
    setLighterAccountIndex("0");
    setLighterApiKey("");
    setLighterSubaccountIndex("");
    setVantaAccountId("");
    setVantaOrderlyKey("");
    setVantaOrderlySecret("");
    setVantaSubaccountIndex("");
    setRobinhoodWallet("");
    setRobinhoodPrivateKey("");
    setRobinhoodSubaccountIndex("");
    setArcusApiKey("");
    setArcusSecret("");
    setKrakenSpotApiKey("");
    setKrakenSpotSecret("");
    setKrakenFuturesApiKey("");
    setKrakenFuturesSecret("");
    setPolymarketApiKey("");
    setPolymarketSecret("");
    setPolymarketPassphrase("");
    setKalshiApiKeyId("");
    setKalshiPrivateKey("");
  };

  const handleLockVault = async () => {
    try {
      const s = await api.lockVault();
      setVaultStatus(s);
      setConfiguredVenues([]);
      clearCredentialInputs();
      setShowVaultModal(true);
    } catch (err) {
      // Refused while live positions are open — surface the reason, don't
      // pretend the vault locked.
      const msg = err instanceof Error ? err.message : String(err);
      console.error("Lock refused:", msg);
      setErrorMsg(msg);
    }
  };

  /** Reloads everything an account-level save affects so the card's own
   *  Save Changes button is sufficient — no overall save needed:
   *  account lists, venue credential-presence, balances, and live statuses. */
  const handleAccountsChanged = () => {
    setAccountsEpoch((e) => e + 1);
    void api
      .getEngineSettings()
      .then((s) => setConfiguredVenues(s.configured_venues || []))
      .catch(() => undefined);
    void loadVenueBalances();
    void api
      .getSystemStatus()
      .then((sys) => {
        if (sys?.active_venues) setLiveVenues(sys.active_venues);
      })
      .catch(() => undefined);
  };

  const handleRemoveHsmToken = async () => {
    if (!configuredVenues.includes("remote_hsm")) return;
    if (
      !window.confirm("Remove the encrypted HSM token? This cannot be undone.")
    ) {
      return;
    }
    setErrorMsg(null);
    try {
      await api.deleteExchangeCredentials("remote_hsm");
      const sanitized = await api.getEngineSettings();
      setConfiguredVenues(sanitized.configured_venues || []);
      clearCredentialInputs();
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    }
  };

  const handleCheckForUpdates = () => {
    setCheckingUpdate(true);
    setUpdateMsg(null);
    setTimeout(() => {
      setCheckingUpdate(false);
      setUpdateMsg("System up to date. Arbitrageur v1.2.0 is currently the latest stable release.");
    }, 1200);
  };

  const isVenueConfigured = (venue: string): boolean => {
    const configured = (id: string) => configuredVenues.includes(id);
    switch (venue) {
      case "hyperliquid":
        return isExchangeEnabled("hyperliquid") && (configured("hyperliquid") || Boolean(hyperliquidWallet || hyperliquidPrivateKey) || hasConfiguredAccount("hyperliquid"));
      case "lighter":
        return isExchangeEnabled("lighter") && (configured("lighter") || Boolean(lighterApiKey) || hasConfiguredAccount("lighter"));
      case "vanta":
        return isExchangeEnabled("vanta") && (configured("vanta") || Boolean(vantaOrderlyKey || vantaAccountId) || hasConfiguredAccount("vanta"));
      case "arcus":
        return isExchangeEnabled("arcus") && (configured("arcus") || Boolean(arcusApiKey) || hasConfiguredAccount("arcus"));
      case "popdex":
        return isExchangeEnabled("popdex") && (configured("popdex") || hasConfiguredAccount("popdex"));
      case "robinhood":
        return isExchangeEnabled("robinhood") && (configured("robinhood") || Boolean(robinhoodWallet || robinhoodPrivateKey) || hasConfiguredAccount("robinhood"));
      case "kraken":
        return isExchangeEnabled("kraken") && (
          configured("kraken_spot") || configured("kraken_futures") ||
          (krakenSpotEnabled && Boolean(krakenSpotApiKey)) ||
          (krakenFuturesEnabled && Boolean(krakenFuturesApiKey)) ||
          hasConfiguredAccount("kraken")
        );
      case "polymarket":
        return isExchangeEnabled("polymarket") && (configured("polymarket") || Boolean(polymarketApiKey || polymarketSecret) || hasConfiguredAccount("polymarket"));
      case "kalshi":
        return isExchangeEnabled("kalshi") && (configured("kalshi") || Boolean(kalshiApiKeyId || kalshiPrivateKey) || hasConfiguredAccount("kalshi"));
      default:
        // Keyless public venues (incl. all extended venues): the enable
        // toggle plus any account/configured record is the configuration.
        return isExchangeEnabled(venue as ExchangeId) && (configured(venue) || hasConfiguredAccount(venue));
    }
  };

  const configuredCount = [
    isVenueConfigured("hyperliquid"),
    isVenueConfigured("lighter"),
    isVenueConfigured("vanta"),
    isVenueConfigured("arcus"),
    isVenueConfigured("robinhood"),
    isVenueConfigured("kraken"),
    isVenueConfigured("popdex"),
    isVenueConfigured("polymarket"),
    isVenueConfigured("kalshi"),
  ].filter(Boolean).length;


  const isVaultLocked = !api.isTauri || !vaultStatus?.is_unlocked;
  const isVaultInitializing = vaultStatus?.is_initialized === false;

  return (
    <div className="flex flex-col h-full min-h-0 overflow-hidden select-none bg-surface">
      {/* Top Section Navigation Bar */}
      <div className="border-b border-surface-border bg-slate-50/80 shrink-0 flex items-stretch select-none">
        {/* Navigation Tabs with dividers — 5 equal columns spanning the full width */}
        <div className="grid grid-cols-5 flex-1 items-stretch divide-x divide-surface-border">
          <button
            type="button"
            onClick={() => setActiveTab("execution")}
            className={`relative h-10 flex items-center justify-center space-x-2 px-2.5 sm:px-2.5 text-[11px] font-mono transition select-none group cursor-pointer ${
              activeTab === "execution"
                ? "text-accent-strong font-bold bg-surface"
                : "text-slate-600 hover:text-slate-900 hover:bg-slate-100/70 font-medium"
            }`}
          >
            <Sliders
              className={`w-3 h-3 transition shrink-0 ${
                activeTab === "execution" ? "text-accent-strong" : "text-muted group-hover:text-slate-600"
              }`}
            />
            <span>Execution</span>
            {activeTab === "execution" && (
              <div className="absolute bottom-0 left-0 right-0 h-[2.5px] bg-accent" />
            )}
          </button>

          <button
            type="button"
            onClick={() => setActiveTab("safety")}
            className={`relative h-10 flex items-center justify-center space-x-2 px-2.5 sm:px-2.5 text-[11px] font-mono transition select-none group cursor-pointer ${
              activeTab === "safety"
                ? "text-accent-strong font-bold bg-surface"
                : "text-slate-600 hover:text-slate-900 hover:bg-slate-100/70 font-medium"
            }`}
          >
            <ShieldAlert
              className={`w-3 h-3 transition shrink-0 ${
                activeTab === "safety" ? "text-accent-strong" : "text-muted group-hover:text-slate-600"
              }`}
            />
            <span>Risk Control</span>
            {activeTab === "safety" && (
              <div className="absolute bottom-0 left-0 right-0 h-[2.5px] bg-accent" />
            )}
          </button>

          <button
            type="button"
            onClick={() => setActiveTab("exchanges")}
            className={`relative h-10 flex items-center justify-center space-x-2 px-2.5 sm:px-2.5 text-[11px] font-mono transition select-none group cursor-pointer ${
              activeTab === "exchanges"
                ? "text-accent-strong font-bold bg-surface"
                : "text-slate-600 hover:text-slate-900 hover:bg-slate-100/70 font-medium"
            }`}
          >
            <Key
              className={`w-3 h-3 transition shrink-0 ${
                activeTab === "exchanges" ? "text-accent-strong" : "text-muted group-hover:text-slate-600"
              }`}
            />
            <span>Exchanges</span>
            {activeTab === "exchanges" && (
              <div className="absolute bottom-0 left-0 right-0 h-[2.5px] bg-accent" />
            )}
          </button>


          <button
            type="button"
            onClick={() => setActiveTab("network")}
            className={`relative h-10 flex items-center justify-center space-x-2 px-2.5 sm:px-2.5 text-[11px] font-mono transition select-none group cursor-pointer ${
              activeTab === "network"
                ? "text-accent-strong font-bold bg-surface"
                : "text-slate-600 hover:text-slate-900 hover:bg-slate-100/70 font-medium"
            }`}
          >
            <Globe
              className={`w-3 h-3 transition shrink-0 ${
                activeTab === "network" ? "text-accent-strong" : "text-muted group-hover:text-slate-600"
              }`}
            />
            <span>Network &amp; VPS</span>
            {activeTab === "network" && (
              <div className="absolute bottom-0 left-0 right-0 h-[2.5px] bg-accent" />
            )}
          </button>

          <button
            type="button"
            onClick={() => setActiveTab("about")}
            className={`relative h-10 flex items-center justify-center space-x-2 px-2.5 sm:px-2.5 text-[11px] font-mono transition select-none group cursor-pointer ${
              activeTab === "about"
                ? "text-accent-strong font-bold bg-surface"
                : "text-slate-600 hover:text-slate-900 hover:bg-slate-100/70 font-medium"
            }`}
          >
            <Info
              className={`w-3 h-3 transition shrink-0 ${
                activeTab === "about" ? "text-accent-strong" : "text-muted group-hover:text-slate-600"
              }`}
            />
            <span>About</span>
            {activeTab === "about" && (
              <div className="absolute bottom-0 left-0 right-0 h-[2.5px] bg-accent" />
            )}
          </button>
        </div>

        {/* Vault lock/unlock is hoisted into the modal header via portal
            (headerActionsEl); tab-scoped actions render only when present
            so the tabs keep the full row width otherwise. */}
        {activeTab === "network" &&
          !isVaultLocked &&
          configuredVenues.includes("remote_hsm") && (
            <div className="flex items-center space-x-2 py-2 pr-5 sm:pr-6 border-l border-surface-border">
              <button
                type="button"
                onClick={handleRemoveHsmToken}
                className="px-2 py-2 rounded-lg border border-loss-border bg-surface hover:bg-loss-dim text-loss text-xs font-mono flex items-center space-x-1.5 transition shadow-2xs cursor-pointer"
                title="Permanently remove the stored HSM token from the vault"
              >
                <Trash2 className="w-3.5 h-3.5 text-loss" />
                <span>Remove HSM Token</span>
              </button>
            </div>
          )}
      </div>

      {/* Main Content Form Area */}
      <div className="flex-1 flex flex-col min-w-0 h-full overflow-hidden bg-surface">
        <form onSubmit={handleSave} className="flex flex-col h-full min-h-0">

          {activeTab === "exchanges" ? (
            <div className="relative flex-1 flex flex-row min-h-0 overflow-hidden">
              {/* Left Panel: Exchanges Selector in Alphabetical Order */}
              <div className="w-56 sm:w-64 shrink-0 border-r border-surface-border bg-slate-50/50 flex flex-col min-h-0">
                <div className="p-3 border-b border-surface-border bg-slate-100/60 flex items-center justify-between shrink-0">
                  <span className="text-xs font-mono font-bold text-slate-700 tracking-wider">
                    Exchanges ({configuredCount}/{EXCHANGES_ALPHABETICAL.length})
                  </span>
                  <div className="flex items-center gap-1.5">
                    <div
                      role="group"
                      aria-label="Exchange list filter"
                      title="Switch between all exchanges and favorites only"
                      className="h-7 flex items-stretch rounded-lg border border-surface-border bg-surface overflow-hidden"
                    >
                      <button
                        type="button"
                        onClick={() => setExchangeFilter("all")}
                        aria-pressed={exchangeFilter === "all"}
                        title="Show all exchanges"
                        className={`px-2 flex items-center gap-1.5 text-[11px] font-mono font-bold transition cursor-pointer ${
                          exchangeFilter === "all"
                            ? "bg-accent text-white"
                            : "text-slate-500 hover:text-slate-800 hover:bg-slate-100/70"
                        }`}
                      >
                        <span>All</span>
                      </button>
                      <button
                        type="button"
                        onClick={() => setExchangeFilter("favorites")}
                        aria-pressed={exchangeFilter === "favorites"}
                        title={`Favorites only (${favoriteExchanges.length} starred)`}
                        className={`px-2 flex items-center gap-1.5 text-[11px] font-mono font-bold border-l border-surface-border transition cursor-pointer ${
                          exchangeFilter === "favorites"
                            ? "bg-accent text-white"
                            : "text-slate-500 hover:text-slate-800 hover:bg-slate-100/70"
                        }`}
                      >
                        <Star className={`w-3.5 h-3.5 ${exchangeFilter === "favorites" ? "fill-current" : ""}`} />
                      </button>
                    </div>
                    <button
                      type="button"
                      onClick={async () => {
                        setPinging(true);
                        try {
                          const status = await api.refreshVenueLatencies();
                          if (status.active_venues) setLiveVenues(status.active_venues);
                        } catch (err: unknown) {
                          setErrorMsg(err instanceof Error ? err.message : String(err));
                        } finally {
                          setPinging(false);
                        }
                      }}
                      disabled={pinging}
                      title="Measure HTTP round-trip to all enabled venue API endpoints now"
                      aria-label="Measure venue API latency now"
                      className="h-7 w-7 bg-surface hover:bg-surface-raised border border-surface-border text-slate-600 hover:text-slate-900 rounded-lg transition flex items-center justify-center disabled:opacity-50 cursor-pointer"
                    >
                      <RefreshCw className={`w-3.5 h-3.5 ${pinging ? "animate-spin" : ""}`} />
                    </button>
                  </div>
                </div>

                <div className="flex-1 overflow-y-auto p-2 space-y-1">
                  {visibleExchanges.length === 0 && (
                    <div className="px-3 py-6 text-center text-[11px] font-mono text-muted">
                      No favorites yet — star an exchange to pin it here.
                    </div>
                  )}
                  {groupExchangesBySection(visibleExchanges).map(({ group, venues }) => (
                    <div key={group}>
                      {exchangeFilter !== "favorites" && groupExchangesBySection(visibleExchanges).length > 1 && (
                        <div className="px-2 pt-3 pb-1 text-[10px] font-mono font-bold tracking-wider text-muted uppercase">
                          {group}
                        </div>
                      )}
                      {venues.map((ex) => {
                    const isSelected = selectedExchange === ex.id;
                    const isEnabled = isExchangeEnabled(ex.id);
                    const bal = getVenueBalance(ex.id);
                    const latency = getVenueLatency(ex.id);
                    // Backend admission status is authoritative — an enabled
                    // venue without an adapter shows an amber dot, not a
                    // green "connected" dot.
                    const vStatus = getVenueStatus(ex.id);
                    const isStreaming = vStatus === "READ_ONLY";
                    const noAdapter = vStatus === "UNSUPPORTED";
                    const showLatency = isEnabled && latency != null;
                    const showBalance = bal != null;
                    return (
                      <button
                        key={ex.id}
                        type="button"
                        onClick={() => setSelectedExchange(ex.id)}
                        className={`w-full text-left p-2.5 rounded-lg text-xs font-mono transition flex items-center justify-between group cursor-pointer border ${
                          isSelected
                            ? "bg-accent-dim text-accent-strong border-accent-line shadow-2xs"
                            : "bg-surface hover:bg-slate-100/80 text-slate-700 border-surface-border/60 hover:border-surface-border"
                        }`}
                      >
                        <div className="min-w-0 pr-2">
                          <div className="flex items-center space-x-2">
                            <span
                              className={`w-2 h-2 rounded-full shrink-0 ${
                                isStreaming
                                  ? "bg-emerald-500"
                                  : noAdapter
                                  ? "bg-amber-400"
                                  : "bg-slate-300"
                              }`}
                              title={
                                isStreaming
                                  ? "Market quotes are integrated and active"
                                  : noAdapter
                                  ? "Enabled, but market quotes are not integrated"
                                  : vStatus === "OFFLINE"
                                  ? "Not enabled"
                                  : undefined
                              }
                            />
                            <VenueLogo venueId={ex.id} name={ex.name} size={22} />
                            <span className={`font-semibold truncate ${isSelected ? "text-accent-strong font-bold" : "text-slate-900"}`}>
                              {ex.name}
                            </span>
                            <span
                              role="checkbox"
                              aria-checked={favoriteExchanges.includes(ex.id)}
                              title={favoriteExchanges.includes(ex.id) ? "Remove from favorites" : "Add to favorites"}
                              onClick={(e) => {
                                e.stopPropagation();
                                toggleFavorite(ex.id);
                              }}
                              className={`shrink-0 cursor-pointer transition ${
                                favoriteExchanges.includes(ex.id)
                                  ? "text-amber-600"
                                  : "text-slate-300 opacity-0 group-hover:opacity-100 hover:text-amber-400"
                              }`}
                            >
                              <Star className={`w-2.5 h-2.5 ${favoriteExchanges.includes(ex.id) ? "fill-amber-500" : ""}`} />
                            </span>
                          </div>
                        </div>
                        {(showLatency || showBalance) && (
                        <div className="text-right shrink-0">
                          {showLatency && (
                          <span
                            className={`text-[11px] font-mono block font-semibold ${
                              noAdapter
                                ? "text-warn-strong"
                                : latencyClass(latency as number)
                            }`}
                            title={
                              isStreaming
                                ? `Market data active · measured API round-trip: ${latency} ms`
                                : noAdapter
                                ? `API reachable (${latency} ms), but market quotes are not integrated`
                                : undefined
                            }
                          >
                            {`${latency} ms`}
                          </span>
                          )}
                          {showBalance && bal && (
                          <span
                            className="text-[11px] font-mono block font-bold text-slate-800"
                            title={
                              bal.is_live
                                ? "Real exchange equity"
                                : "Testnet venue equity — demo funds, not live exchange equity"
                            }
                          >
                            {bal.is_live
                              ? `$${bal.equity_usd.toLocaleString("en-US", { maximumFractionDigits: 0 })}`
                              : `Testnet $${bal.equity_usd.toLocaleString("en-US", { maximumFractionDigits: 0 })}`}
                          </span>
                          )}
                        </div>
                        )}
                      </button>
                    );
                      })}
                    </div>
                  ))}
                </div>

              </div>

              {/* Right Content Pane: Single Selected Exchange */}
              <div className="flex-1 overflow-y-auto p-4 space-y-4 min-w-0">
                {errorMsg && (
                  <div className="p-4 bg-loss-dim border border-loss-border text-loss rounded-xl text-xs font-mono flex items-center space-x-2">
                    <AlertCircle className="w-4 h-4 shrink-0" />
                    <span>{errorMsg}</span>
                  </div>
                )}


                {/* Non-blocking Vault Locked Banner */}
                {isVaultLocked && api.isTauri && (
                  <div className="p-3.5 rounded-xl bg-amber-500/10 border border-amber-500/30 flex items-center justify-between shadow-2xs">
                    <div className="flex items-center space-x-3">
                      <div className="w-8 h-8 rounded-lg bg-amber-500/20 border border-amber-500/40 flex items-center justify-center text-amber-700 shrink-0">
                        <Lock className="w-4 h-4" />
                      </div>
                      <div>
                        <h4 className="text-xs font-bold font-mono text-slate-900">
                          {isVaultInitializing ? "Master Password Not Created" : "Secure Vault is Locked"}
                        </h4>
                        <p className="text-[11px] text-slate-600 font-sans mt-0.5">
                          {isVaultInitializing
                            ? "Create a master password to encrypt and manage exchange API keys."
                            : "Unlock your vault with your master password to view, add, or edit live API credentials."}
                        </p>
                      </div>
                    </div>
                    <button
                      type="button"
                      onClick={() => setShowVaultModal(true)}
                      className="btn-accent px-2 py-2 text-xs font-mono font-bold flex items-center space-x-1.5 shrink-0 ml-3 cursor-pointer"
                    >
                      <Unlock className="w-3.5 h-3.5" />
                      <span>{isVaultInitializing ? "Create Password" : "Unlock Vault"}</span>
                    </button>
                  </div>
                )}

            {/* Exchange: Kraken (Spot & Futures Multi-Market) */}
            {selectedExchange === "kraken" && (
              <div className="space-y-4">
                <div className="p-2.5 bg-blue-50/70 border border-blue-200 rounded-lg text-xs font-mono text-slate-600">
                  💡 Kraken maintains separate API infrastructure for Spot and Futures markets. Configure accounts independently for each market.
                </div>

                {/* Kraken Spot */}
                <div className="space-y-2">
                  <div className="flex items-center space-x-2">
                    <span className="text-xs font-bold font-mono text-slate-900">1. Kraken Spot Trading API</span>
                    <span className="text-xs font-mono px-1.5 py-0.5 rounded-md bg-slate-200 text-slate-700">api.kraken.com</span>
                  </div>
                  <VenueAccountsSection
                    venue="kraken_spot"
                    locked={isVaultLocked}
                    environment={tradingMode}
                    bal={getVenueBalance("kraken")}
                    lane={getVenueLane("kraken")}
                    fallbackEnabled={krakenSpotEnabled}
                    onFallbackChange={setKrakenSpotEnabled}
                    refreshKey={accountsEpoch}
                    onChanged={handleAccountsChanged}
                  />
                </div>

                {/* Kraken Futures */}
                <div className="space-y-2">
                  <div className="flex items-center space-x-2">
                    <span className="text-xs font-bold font-mono text-slate-900">2. Kraken Futures Trading API</span>
                    <span className="text-xs font-mono px-1.5 py-0.5 rounded-md bg-indigo-50 text-indigo-700 border border-indigo-200">futures.kraken.com</span>
                  </div>
                  <VenueAccountsSection
                    venue="kraken_futures"
                    locked={isVaultLocked}
                    environment={tradingMode}
                    bal={getVenueBalance("kraken")}
                    lane={getVenueLane("kraken")}
                    fallbackEnabled={krakenFuturesEnabled}
                    onFallbackChange={setKrakenFuturesEnabled}
                    refreshKey={accountsEpoch}
                    onChanged={handleAccountsChanged}
                  />
                </div>
              </div>
            )}

            {/* Standard Exchanges */}
            {selectedExchange === "popdex" && (
              <div className="p-3.5 bg-surface border border-surface-border rounded-lg shadow-2xs space-y-2">
                <label className="text-slate-800 font-bold block text-xs font-mono">
                  Network
                </label>
                <p className="text-[11px] text-slate-500 font-sans leading-normal">
                  Mainnet (Morph Tachyon chain 2184) uses real funds and live prices.
                  Validate automation on testnet (chain 34952 + test USDT faucet) first —
                  contracts, functions and flow are identical, only RPC and app host change.
                </p>
                <div className="flex gap-2">
                  {(["mainnet", "testnet"] as const).map((n) => (
                    <button
                      key={n}
                      type="button"
                      onClick={() => setPopdexNetwork(n)}
                      aria-pressed={popdexNetwork === n}
                      className={`px-3 py-1.5 rounded-lg text-xs font-mono font-bold border transition cursor-pointer ${
                        popdexNetwork === n
                          ? "bg-accent text-white border-accent"
                          : "text-slate-500 border-surface-border hover:text-slate-800 hover:bg-slate-100/70"
                      }`}
                    >
                      {n === "mainnet" ? "Mainnet" : "Testnet"}
                    </button>
                  ))}
                </div>
              </div>
            )}

            {selectedExchange !== "kraken" && (
              <VenueAccountsSection
                venue={selectedExchange}
                locked={isVaultLocked}
                environment={tradingMode}
                bal={getVenueBalance(selectedExchange)}
                lane={getVenueLane(selectedExchange)}
                fallbackEnabled={venueFlagEnabled(selectedExchange)}
                onFallbackChange={(en) => setVenueFlagEnabled(selectedExchange, en)}
                refreshKey={accountsEpoch}
                onChanged={handleAccountsChanged}
              />
            )}


              </div>
            </div>
          ) : (
            <div className="flex-1 overflow-y-auto p-4 space-y-4">
              {errorMsg && (
                <div className="p-4 bg-loss-dim border border-loss-border text-loss rounded-xl text-xs font-mono flex items-center space-x-2">
                  <AlertCircle className="w-4 h-4 shrink-0" />
                  <span>{errorMsg}</span>
                </div>
              )}


        {/* =================================================================== */}
        {/* SECTION 3: EXECUTION MODES & WARM-UP                               */}
        {/* =================================================================== */}
        {activeTab === "execution" && (
          <div className="max-w-4xl mx-auto w-full space-y-4 text-xs font-mono">
            {/* Trading Mode */}
            <div className="p-3.5 bg-surface border border-surface-border rounded-lg shadow-2xs space-y-3">
              <div className="space-y-2 min-w-0">
                <label className="text-slate-800 font-bold text-xs flex items-center">
                  Trading Mode
                  <InfoTooltip
                    position="bottom"
                    text={"Testnet Trading executes real orders against each venue's public demo/test endpoints — order flow, signing, and reconciliation run exactly like production, with demo balances instead of real funds. Venues without a public testnet are unavailable in this mode.\n\nLive Trading connects your exchange accounts for real-money execution, account monitoring, and risk-reducing closes such as flattening existing positions. Order status is reconciled at startup and continuously thereafter. Strategy entries are off by default and require the explicit opt-in in Safety with a per-leg notional cap. Live mode requires an unlocked credential vault and at least two submission-capable venues."}
                  />
                </label>
                <p className="text-xs text-slate-500">
                  {hasApiConnectivity
                    ? "Both modes require exchange API connectivity for market data."
                    : "Unavailable — no exchange API is connected. Enable a venue with working connectivity in the Exchanges tab first."}
                </p>

                <div className="flex items-center space-x-4 pt-1">
                  <label
                    className={`flex items-center space-x-2 ${hasApiConnectivity ? "cursor-pointer" : "cursor-not-allowed opacity-60"}`}
                    title={hasApiConnectivity ? undefined : "Requires at least one connected exchange API for market data"}
                  >
                    <input
                      type="radio"
                      name="trading_mode"
                      value="testnet"
                      checked={tradingMode === "testnet"}
                      onChange={() => setTradingMode("testnet")}
                      disabled={!hasApiConnectivity}
                      className="text-accent-strong focus:ring-accent"
                    />
                    <span className="font-semibold text-slate-800">Testnet Trading</span>
                  </label>
                  <label
                    className={`flex items-center space-x-2 ${
                      canSwitchToLive || !vaultStatus?.is_unlocked ? "cursor-pointer" : "cursor-not-allowed opacity-60"
                    }`}
                    title={
                      !vaultStatus?.is_unlocked
                        ? "Vault is locked. Click to enter password and unlock the vault."
                        : submissionReadyVenues.length < 2
                        ? `Requires at least 2 submission-capable venues (currently ${submissionReadyVenues.length}: ${submissionReadyVenues.map(v => v.name).join(", ") || "none"}). Configure and enable API keys in the Exchanges tab.`
                        : "Switch to Live Trading"
                    }
                    onClick={() => {
                      if (!vaultStatus?.is_unlocked && api.isTauri) {
                        setShowVaultModal(true);
                      }
                    }}
                  >
                    <input
                      type="radio"
                      name="trading_mode"
                      value="live"
                      checked={tradingMode === "live"}
                      disabled={!canSwitchToLive && (vaultStatus?.is_unlocked ?? false)}
                      onChange={() => {
                        if (!vaultStatus?.is_unlocked && api.isTauri) {
                          setShowVaultModal(true);
                          return;
                        }
                        if (canSwitchToLive) {
                          if (!liveRiskAgreed && tradingMode !== "live") {
                            setShowLiveRiskModal(true);
                          } else {
                            setTradingMode("live");
                          }
                        }
                      }}
                      className="text-loss focus:ring-loss"
                    />
                    <span className="font-semibold text-slate-800 flex items-center gap-1.5">
                      Live Trading
                    </span>
                  </label>
                </div>

                <p className="text-xs text-warn-strong">
                  Live Trading covers account monitoring, risk-reducing closes, and strategy entries when the opt-in below is enabled. Entries stay off by default — enable them explicitly in Risk Control with a per-leg notional cap.
                </p>

              </div>

              <div className="pt-3 border-t border-surface-border space-y-1.5">
                <p className="text-xs text-muted leading-snug">
                  Testnet uses each venue's own demo balance — fund accounts on the venue's testnet faucet, not here. Balances, positions, and order flow are real venue state, not simulation.
                </p>
              </div>
            </div>

            {/* Global Auto Position Exit — single limit-exit policy */}
            <div className="p-3.5 bg-surface border border-surface-border rounded-lg shadow-2xs space-y-3">
              <div className="space-y-1 min-w-0">
                <label className="text-slate-800 font-bold text-xs flex items-center">
                  Auto Position Exit
                  <InfoTooltip
                    position="bottom"
                    text={"The shared limit-exit policy. Enabled by default: reduce-exit paths (strategy flattens, the Portfolio Monitor's Auto Limit mode, scheduled auto exits, residual auto-flattens) first work a pegged limit order — mid-spread by default, re-priced on timeout. The system itself decides limit vs market per attempt: the calm gates (max spread / fast volatility) skip straight to market when the book is too volatile, and the try-count fallback market-closes any remainder, so exposure is never left open.\n\nEach strategy chooses in its own configurator: Inherit (this policy), Custom (its own params), or Off (always market). Kill-switch and explicit emergency flattens always stay market — this never slows a panic exit."}
                  />
                </label>
                <p className="text-xs text-slate-500">
                  Single place to manage the limit-exit method. Strategies set to “Inherit” follow this policy; each can override or opt out in its own configurator.
                </p>
              </div>
              <div className="flex items-center justify-between gap-3">
                <label className="text-slate-600 block text-xs">Enable limit exits globally</label>
                <button
                  type="button"
                  role="switch"
                  aria-checked={passiveUnwind.enabled}
                  disabled={loading}
                  onClick={() => setPassiveUnwind({ ...passiveUnwind, enabled: !passiveUnwind.enabled })}
                  className={`relative h-5 w-9 rounded-full transition cursor-pointer ${passiveUnwind.enabled ? "bg-accent" : "bg-slate-300"}`}
                >
                  <span className={`absolute top-0.5 h-4 w-4 rounded-full bg-white shadow transition-all ${passiveUnwind.enabled ? "left-4.5" : "left-0.5"}`} />
                </button>
              </div>
              <div className="flex items-center justify-between gap-3 border-t border-surface-border/60 pt-3">
                <label className="text-slate-600 block text-xs" title="How long a still-open pair position is held before the auto-exit supervisor unwinds it on timeout — convergence and stop exits can close it sooner. Applies to futures-strategy pairs; radar-bot positions use their own Hold timeout ×">
                  Max hold before auto-exit (min)
                </label>
                <input type="number" min="1" max="10080" step="1" disabled={loading} value={autoExitMaxHoldMin}
                  onChange={(e) => setAutoExitMaxHoldMin(parseInt(e.target.value, 10) || 0)}
                  className="w-24 shrink-0 px-3 py-1.5 bg-surface-raised border border-surface-border rounded-lg text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent" />
              </div>
              <div className={`grid grid-cols-1 sm:grid-cols-2 gap-3 ${passiveUnwind.enabled ? "" : "opacity-50 pointer-events-none"}`}>
                <div className="flex items-center justify-between gap-3">
                  <label className="text-slate-600 block text-xs" title="Peg inside the spread: 0 = aggressive touch (fill odds like market), 50 = mid, 100 = passive edge (best price, lowest touch odds)">
                    Peg percentile (0–100)
                  </label>
                  <input type="number" min="0" max="100" step="1" disabled={loading || !passiveUnwind.enabled} value={passiveUnwind.spreadPercentile}
                    onChange={(e) => setPassiveUnwind({ ...passiveUnwind, spreadPercentile: parseFloat(e.target.value) || 0 })}
                    className="w-24 shrink-0 px-3 py-1.5 bg-surface-raised border border-surface-border rounded-lg text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent" />
                </div>
                <div className="flex items-center justify-between gap-3">
                  <label className="text-slate-600 block text-xs" title="Seconds each limit attempt works before it is cancelled and re-priced">
                    Attempt timeout (s)
                  </label>
                  <input type="number" min="1" max="300" step="1" disabled={loading || !passiveUnwind.enabled} value={passiveUnwind.timeoutSecs}
                    onChange={(e) => setPassiveUnwind({ ...passiveUnwind, timeoutSecs: parseInt(e.target.value, 10) || 0 })}
                    className="w-24 shrink-0 px-3 py-1.5 bg-surface-raised border border-surface-border rounded-lg text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent" />
                </div>
                <div className="flex items-center justify-between gap-3">
                  <label className="text-slate-600 block text-xs" title="Limit attempts before the exit falls back to a market order">
                    Max tries
                  </label>
                  <input type="number" min="1" max="10" step="1" disabled={loading || !passiveUnwind.enabled} value={passiveUnwind.maxTries}
                    onChange={(e) => setPassiveUnwind({ ...passiveUnwind, maxTries: parseInt(e.target.value, 10) || 0 })}
                    className="w-24 shrink-0 px-3 py-1.5 bg-surface-raised border border-surface-border rounded-lg text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent" />
                </div>
                <div className="flex items-center justify-between gap-3">
                  <label className="text-slate-600 block text-xs" title="Calm gate: skip the passive attempt when the live spread is wider than this (bps)">
                    Max spread (bps)
                  </label>
                  <input type="number" min="0.1" step="any" disabled={loading || !passiveUnwind.enabled} value={passiveUnwind.maxSpreadBps}
                    onChange={(e) => setPassiveUnwind({ ...passiveUnwind, maxSpreadBps: parseFloat(e.target.value) || 0 })}
                    className="w-24 shrink-0 px-3 py-1.5 bg-surface-raised border border-surface-border rounded-lg text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent" />
                </div>
                <div className="flex items-center justify-between gap-3">
                  <label className="text-slate-600 block text-xs" title="Calm gate: skip the passive attempt when fast volatility exceeds this (bps)">
                    Max fast vol (bps)
                  </label>
                  <input type="number" min="0.1" step="any" disabled={loading || !passiveUnwind.enabled} value={passiveUnwind.maxFastVolBps}
                    onChange={(e) => setPassiveUnwind({ ...passiveUnwind, maxFastVolBps: parseFloat(e.target.value) || 0 })}
                    className="w-24 shrink-0 px-3 py-1.5 bg-surface-raised border border-surface-border rounded-lg text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent" />
                </div>
              </div>
            </div>

            {/* Historical Price Data */}
            <div className="p-3.5 bg-surface border border-surface-border rounded-lg shadow-2xs space-y-3">
              <div className="space-y-1 min-w-0">
                <label className="text-slate-800 font-bold block text-xs">Historical Price Data</label>
                <p className="text-xs text-slate-500">
                  How much top-of-book history to keep. Empty means no limit. Time and
                  size limits compose — whichever binds first wins.
                </p>
              </div>
              <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
                <div className="flex items-center justify-between gap-3">
                  <div className="flex-1 min-w-0">
                    <label className="text-slate-600 block font-mono text-xs">
                      Max history days
                    </label>

                  </div>
                  <input
                    type="number"
                    min="0"
                    step="1"
                    disabled={loading}
                    value={maxHistoryDays}
                    placeholder="No limit"
                    onChange={(e) => setMaxHistoryDays(e.target.value)}
                    className="w-24 shrink-0 px-3 py-1.5 bg-surface-raised border border-surface-border rounded-lg text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent"
                  />
                </div>
                <div className="flex items-center justify-between gap-3">
                  <div className="flex-1 min-w-0">
                    <label className="text-slate-600 block font-mono text-xs">
                      Max storage GB
                    </label>

                  </div>
                  <input
                    type="number"
                    min="0"
                    step="any"
                    disabled={loading}
                    value={maxStorageGb}
                    placeholder="No limit"
                    onChange={(e) => setMaxStorageGb(e.target.value)}
                    className="w-24 shrink-0 px-3 py-1.5 bg-surface-raised border border-surface-border rounded-lg text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent"
                  />
                </div>
              </div>
              <div className="flex flex-wrap items-center gap-x-4 gap-y-1 border-t border-surface-border pt-2.5 text-[11px] text-slate-500">
                <span title="Oldest → newest top-of-book sample across every symbol and venue">
                  Retained{" "}
                  <span className="font-semibold text-slate-700 tabular-nums">
                    {storageStats
                      ? formatRetainedSpan(storageStats.oldest_sample_ts_ms, storageStats.newest_sample_ts_ms)
                      : "—"}
                  </span>
                </span>
                <span title="Whole local database file — the GB limit above enforces on this">
                  Stored{" "}
                  <span className="font-semibold text-slate-700 tabular-nums">
                    {storageStats ? formatStorageBytes(storageStats.db_bytes) : "—"}
                  </span>
                </span>
                <span className="text-muted tabular-nums">
                  {storageStats ? `${storageStats.sample_rows.toLocaleString()} samples` : ""}
                </span>
              </div>
            </div>

          </div>
        )}

        {/* =================================================================== */}
        {/* SECTION: SAFETY — CIRCUIT BREAKER + SAFETY NET                      */}
        {/* =================================================================== */}
        {activeTab === "safety" && (
          <div className="max-w-4xl mx-auto w-full space-y-4 text-xs font-mono">
            <div className="p-3.5 bg-surface border border-surface-border rounded-lg shadow-2xs space-y-1">
              <label className="text-slate-800 font-bold text-xs flex items-center gap-2">
                <ShieldAlert className="w-3.5 h-3.5 text-accent-strong" />
                Overall Circuit Breaker & Safety Net
              </label>
              <p className="text-xs text-slate-500">
                Global halt + guardrail limits. Applies live on save — no restart. Tightening never trips by itself; the next equity update enforces it.
              </p>
            </div>

            <div className="p-3.5 bg-surface border border-surface-border rounded-lg shadow-2xs space-y-3">
              <div className="text-slate-800 font-bold text-xs">Circuit Breaker — Halt Limits</div>
              <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
                <div className="flex items-center justify-between gap-3">
                  <label className="text-slate-600 block text-xs" title="Halt when the day's loss from the open baseline reaches this USD amount">
                    Max daily drawdown (USD)
                  </label>
                  <input type="number" min="1" step="any" disabled={loading} value={cbMaxDdUsd}
                    onChange={(e) => setCbMaxDdUsd(parseFloat(e.target.value) || 0)}
                    className="w-28 shrink-0 px-3 py-1.5 bg-surface-raised border border-surface-border rounded-lg text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent" />
                </div>
                <div className="flex items-center justify-between gap-3">
                  <label className="text-slate-600 block text-xs" title="Halt when the day's loss reaches this percent of the open baseline">
                    Max daily drawdown (%)
                  </label>
                  <input type="number" min="0.1" max="100" step="any" disabled={loading} value={cbMaxDdPct}
                    onChange={(e) => setCbMaxDdPct(parseFloat(e.target.value) || 0)}
                    className="w-28 shrink-0 px-3 py-1.5 bg-surface-raised border border-surface-border rounded-lg text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent" />
                </div>
                <div className="flex items-center justify-between gap-3">
                  <label className="text-slate-600 block text-xs" title="Halt after this many consecutive execution errors">
                    Max consecutive errors
                  </label>
                  <input type="number" min="1" max="1000" step="1" disabled={loading} value={cbMaxErrors}
                    onChange={(e) => setCbMaxErrors(parseInt(e.target.value, 10) || 0)}
                    className="w-28 shrink-0 px-3 py-1.5 bg-surface-raised border border-surface-border rounded-lg text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent" />
                </div>
                <div className="flex items-center justify-between gap-3">
                  <label className="text-slate-600 block text-xs" title="Toxic-fill ceiling (bps). Entries whose expected net spread exceeds this are refused outright — an edge that large is a symbol collision or corrupt quote, not alpha.">
                    Max spread divergence (bps)
                  </label>
                  <input type="number" min="1" step="any" disabled={loading} value={cbMaxSpreadBps}
                    onChange={(e) => setCbMaxSpreadBps(parseFloat(e.target.value) || 0)}
                    className="w-28 shrink-0 px-3 py-1.5 bg-surface-raised border border-surface-border rounded-lg text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent" />
                </div>
              </div>
            </div>

            <div className="p-3.5 bg-surface border border-surface-border rounded-lg shadow-2xs space-y-3">
              <div className="text-slate-800 font-bold text-xs">DeltaGuard — Net Exposure Guardrail</div>
              <div className="flex items-center justify-between gap-3">
                <label className="text-slate-600 block text-xs" title="Per-asset signed net exposure tolerance across venues">
                  Max unhedged per asset (USD)
                </label>
                <input type="number" min="1" step="any" disabled={loading} value={deltaMaxUnhedged}
                  onChange={(e) => setDeltaMaxUnhedged(parseFloat(e.target.value) || 0)}
                  className="w-28 shrink-0 px-3 py-1.5 bg-surface-raised border border-surface-border rounded-lg text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent" />
              </div>
              <p className="text-xs text-muted leading-snug border-t border-surface-border/60 pt-1.5">
                Beyond this, exposure-skew warnings fire and executions refuse. Currently $2,500 default.
              </p>
            </div>

            <div className="p-3.5 bg-surface border border-surface-border rounded-lg shadow-2xs space-y-3">
              <div className="text-slate-800 font-bold text-xs">Margin Monitor — Warning Thresholds</div>
              <div className="grid grid-cols-1 sm:grid-cols-3 gap-3">
                <div className="flex items-center justify-between gap-3">
                  <label className="text-slate-600 block text-xs" title="Margin utilization at which warnings start">
                    Warning (%)
                  </label>
                  <input type="number" min="0" max="100" step="any" disabled={loading} value={marginWarnPct}
                    onChange={(e) => setMarginWarnPct(parseFloat(e.target.value) || 0)}
                    className="w-24 shrink-0 px-3 py-1.5 bg-surface-raised border border-surface-border rounded-lg text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent" />
                </div>
                <div className="flex items-center justify-between gap-3">
                  <label className="text-slate-600 block text-xs" title="Margin utilization at which venues flag DEPLETED / blocks entries">
                    Critical (%)
                  </label>
                  <input type="number" min="0" max="100" step="any" disabled={loading} value={marginCritPct}
                    onChange={(e) => setMarginCritPct(parseFloat(e.target.value) || 0)}
                    className="w-24 shrink-0 px-3 py-1.5 bg-surface-raised border border-surface-border rounded-lg text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent" />
                </div>
                <div className="flex items-center justify-between gap-3">
                  <label className="text-slate-600 block text-xs" title="Liquidation buffer below which positions warn">
                    Min liq. distance (%)
                  </label>
                  <input type="number" min="0.1" max="100" step="any" disabled={loading} value={marginMinLiqPct}
                    onChange={(e) => setMarginMinLiqPct(parseFloat(e.target.value) || 0)}
                    className="w-24 shrink-0 px-3 py-1.5 bg-surface-raised border border-surface-border rounded-lg text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent" />
                </div>
              </div>
              <p className="text-xs text-muted leading-snug border-t border-surface-border/60 pt-1.5">
                Warning must stay below critical. Feeds risk snapshots, venue LOW_MARGIN / DEPLETED flags, and pre-trade margin gates.
              </p>
              <div className="flex items-center justify-between gap-3">
                <label className="text-slate-600 block text-xs" title="A venue whose balance read keeps failing holds its last known figures; warn only after this many seconds">
                  Stale-balance warning after (secs)
                </label>
                <input type="number" min="30" max="3600" step="1" disabled={loading} value={staleWarnSecs}
                  onChange={(e) => setStaleWarnSecs(parseInt(e.target.value, 10) || 0)}
                  className="w-24 shrink-0 px-3 py-1.5 bg-surface-raised border border-surface-border rounded-lg text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent" />
              </div>
              <p className="text-xs text-muted leading-snug border-t border-surface-border/60 pt-1.5">
                Until then a blip keeps last-known figures — totals and drawdown never treat it as $0.
              </p>
            </div>

            <div className="p-3.5 bg-surface border border-surface-border rounded-lg shadow-2xs space-y-3">
              <div className="text-slate-800 font-bold text-xs">Leverage — Global Max</div>
              <div className="flex items-center justify-between gap-3">
                <label className="text-slate-600 block text-xs" title="Hard ceiling on every position: effective leverage is min(strategy target, account max, venue max, this)">
                  Max leverage
                </label>
                <div className="flex items-center gap-1.5 shrink-0">
                  <input type="number" min="1" max="100" step="1" disabled={loading} value={maxLeverage}
                    onChange={(e) => setMaxLeverage(Math.trunc(Number(e.target.value)) || 0)}
                    className="w-24 px-3 py-1.5 bg-surface-raised border border-surface-border rounded-lg text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent" />
                  <span className="text-slate-500 text-xs">x</span>
                </div>
              </div>
              <p className="text-xs text-muted leading-snug border-t border-surface-border/60 pt-1.5">
                Caps every strategy (Risk Control default 10x). Per-strategy targets in the configurators can only lower it — never exceed it. Applies live on save.
              </p>
            </div>

            <div className="flex items-start gap-2.5 pt-1 border-t border-slate-200 mt-2">
              <input type="checkbox" id="live-entry-enabled" disabled={loading}
                checked={liveEntryEnabled}
                onChange={(e) => setLiveEntryEnabled(e.target.checked)}
                className="mt-0.5 accent-red-600" />
              <label htmlFor="live-entry-enabled" className="text-xs text-slate-700 leading-snug">
                <span className="font-bold text-red-700">Enable live strategy entries.</span>{" "}
                Permits REAL-money strategy entries — spatial pairs, Spread
                Radar manual entries, funding carry/basis, and single-venue
                triangle cycles — while Trading Mode is Live. Every leg is
                refused above the notional cap below — caps are never
                clamped. Order status is reconciled via authoritative venue
                polling, residual auto-flatten and cancel retry run
                continuously, and a partially-filled entry unwinds the
                filled leg automatically. Residual risk remains: a venue
                outage at the wrong moment can still leave one-sided
                exposure you must close manually. Keep the cap small.
              </label>
            </div>
            <div>
              <label className="text-xs font-semibold text-slate-600">Live max notional per leg (USD)</label>
              <input type="number" min="0" step="1" disabled={loading}
                value={liveEntryMaxNotional}
                onChange={(e) => setLiveEntryMaxNotional(Number(e.target.value))}
                className="w-full mt-1 rounded border border-slate-300 px-2 py-1 text-sm" />
              <p className="text-xs text-slate-500 mt-0.5">
                Hard cap on each leg's qty × entry VWAP. 0 refuses every live
                entry. For a canary set it just above both venues' minimum
                notionals (e.g. 15–25).
              </p>
            </div>

            {liveEntryEnabled && tradingMode === "live" && (
              <div className="rounded border border-red-300 bg-red-50 p-2.5 space-y-2">
                <p className="text-[11px] font-bold text-red-700 tracking-wide">
                  LIVE CANARY — one minimal two-leg entry
                </p>
                <div className="grid grid-cols-2 gap-2">
                  <input className="rounded border border-slate-300 px-2 py-1 text-xs" value={canarySymbol}
                    onChange={(e) => setCanarySymbol(e.target.value)} placeholder="symbol e.g. ETH-USDC" />
                  <input className="rounded border border-slate-300 px-2 py-1 text-xs" type="number" step="any" min="0"
                    value={canaryQty} onChange={(e) => setCanaryQty(Number(e.target.value))} placeholder="qty per leg" />
                  <input className="rounded border border-slate-300 px-2 py-1 text-xs" value={canaryBuyVenue}
                    onChange={(e) => setCanaryBuyVenue(e.target.value)} placeholder="buy venue" />
                  <input className="rounded border border-slate-300 px-2 py-1 text-xs" value={canarySellVenue}
                    onChange={(e) => setCanarySellVenue(e.target.value)} placeholder="sell venue" />
                  <input className="rounded border border-slate-300 px-2 py-1 text-xs" type="number" min="0" max="50"
                    value={canaryLossBps} onChange={(e) => setCanaryLossBps(Number(e.target.value))} placeholder="max accepted loss bps" />
                </div>
                <p className="text-[11px] text-slate-600 leading-snug">
                  Submits ONE IOC pair on real venues with the profit hurdle
                  relaxed down to the accepted loss above (spread+fees are the
                  test cost, ≤ 50 bps). All other checks still apply: notional
                  cap, margin, depth, delta-neutrality, ticker blocking,
                  reconciliation. Pick a symbol with NO open position — a leg
                  fill may stay open and need a manual close.
                </p>
                <button
                  disabled={canaryRunning || loading}
                  onClick={async () => {
                    setCanaryRunning(true);
                    setCanaryResult(null);
                    try {
                      const r = await api.executeSpatialArbitrage(
                        canarySymbol.trim(), canaryBuyVenue.trim(), canarySellVenue.trim(),
                        canaryQty, undefined, undefined, undefined, 1, 1, undefined,
                        canaryLossBps,
                      );
                      setCanaryResult(r.execution_success ? `FILLED: ${r.message}` : `REFUSED: ${r.message}`);
                    } catch (e) {
                      setCanaryResult(`ERROR: ${e instanceof Error ? e.message : String(e)}`);
                    } finally {
                      setCanaryRunning(false);
                    }
                  }}
                  className="w-full rounded bg-red-600 hover:bg-red-700 text-white text-xs font-bold py-1.5 transition disabled:opacity-50"
                >
                  {canaryRunning ? "Submitting real orders…" : "Run live canary entry"}
                </button>
                {canaryResult && (
                  <p className="text-[11px] font-mono whitespace-pre-wrap bg-white rounded border border-slate-200 p-1.5 max-h-28 overflow-auto">{canaryResult}</p>
                )}
              </div>
            )}
          </div>
        )}

        {/* =================================================================== */}
        {/* SECTION 4: NETWORKING & VPS 24/7 DEPLOYMENT                         */}
        {/* =================================================================== */}
        {activeTab === "network" && (
          <div className="bg-surface border border-surface-border rounded-lg p-3.5 space-y-3 shadow-2xs">
            <div className="border-b border-surface-border pb-2.5">
              <h3 className="text-[13px] font-mono font-bold text-slate-900 flex items-center space-x-2">
                <Globe className="w-4 h-4 text-accent-strong" />
                <span>24/7 VPS Background Daemon</span>
              </h3>
              <p className="text-xs text-slate-500 font-mono mt-0.5">
                Deploy the standalone binary headlessly on a cloud server
              </p>
            </div>

            {/* VPS Daemon Deployment Guide */}
            <div className="p-4 bg-slate-50 border border-surface-border rounded-lg space-y-2 text-xs font-mono text-slate-700">
              <h4 className="font-bold text-slate-900 flex items-center space-x-1.5">
                <span>🖥️ Headless VPS 24/7 Deployment Instructions</span>
              </h4>
              <p className="text-xs text-slate-600">
                To run Arbitrageur continuously 24/7 without keeping your local laptop or desktop open, deploy the standalone Rust binary to an Ubuntu/Debian VPS:
              </p>
              <div className="p-2.5 bg-slate-900 text-emerald-400 rounded-lg text-xs overflow-x-auto select-text font-mono">
                <code>./target/release/arbitrageur serve --bind 0.0.0.0 --port 8787</code>
              </div>
              <p className="text-xs text-slate-500">
                The serve command exposes a read-only health endpoint only — no market data ingestion or order execution runs. You can create a systemd service (e.g. <code className="bg-slate-200 px-1 py-0.5 rounded-md">systemctl start arbitrageur</code>) to auto-restart on server reboot.
              </p>
            </div>
          </div>
        )}

        {/* =================================================================== */}
        {/* SECTION 5: ABOUT, CHECK FOR UPDATES & BUY ME A SOLO                 */}
        {/* =================================================================== */}
        {activeTab === "about" && (
          <div className="space-y-4">
            <div className="bg-surface border border-surface-border rounded-lg p-3.5 shadow-2xs space-y-3">
              <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-4 border-b border-surface-border pb-2.5">
                <div className="flex items-center space-x-3">
                  <div className="w-10 h-10 rounded-lg bg-accent-dim border border-accent-line flex items-center justify-center text-accent-strong">
                    <ArbitrageurLogo size={22} className="text-accent-strong" />
                  </div>
                  <div>
                    <h3 className="text-[13px] font-bold font-mono tracking-wider text-slate-900">
                      About Arbitrageur ({version})
                    </h3>
                  </div>
                </div>

                <div className="flex items-center space-x-2">
                  <button
                    type="button"
                    onClick={handleCheckForUpdates}
                    disabled={checkingUpdate}
                    className="btn-secondary font-mono font-medium flex items-center space-x-1.5"
                  >
                    <RefreshCw className={`w-3.5 h-3.5 ${checkingUpdate ? "animate-spin text-accent-strong" : ""}`} />
                    <span>{checkingUpdate ? "Checking..." : "Check for Updates"}</span>
                  </button>

                  <a
                    href={gitRepo}
                    target="_blank"
                    rel="noopener noreferrer"
                    className="px-3.5 py-1.5 bg-surface hover:bg-surface-raised border border-surface-border text-slate-800 rounded-lg text-xs font-mono font-medium transition flex items-center space-x-1.5 shadow-2xs"
                  >
                    <span>Git Repository</span>
                    <ExternalLink className="w-3.5 h-3.5" />
                  </a>
                </div>
              </div>

              {updateMsg && (
                <div className="p-3 bg-profit-dim border border-profit-border text-profit rounded-xl text-xs font-mono flex items-center space-x-2">
                  <CheckCircle2 className="w-4 h-4 text-profit shrink-0" />
                  <span>{updateMsg}</span>
                </div>
              )}

              {/* Sponsor Card: Buy me a SOLO */}
              <div className="p-5 bg-gradient-to-r from-amber-50/80 via-yellow-50/50 to-amber-50/80 border border-amber-200 rounded-lg flex flex-col sm:flex-row items-center justify-between gap-4 shadow-2xs">
                <div className="flex items-center space-x-4">
                  <img
                    src="/solo_can.jpg"
                    alt="Can of Australian SOLO Lemon Soda"
                    className="w-14 h-14 object-contain rounded-lg shadow-2xs border border-amber-200 bg-white"
                  />
                  <div>
                    <h4 className="text-[13px] font-bold font-mono tracking-wider text-slate-900">
                      Enjoying The Tool? Buy Me A SOLO!
                    </h4>
                  </div>
                </div>

                <a
                  href={sponsorLink}
                  target="_blank"
                  rel="noopener noreferrer"
                  className="px-5 py-2.5 bg-amber-500 hover:bg-amber-600 text-white font-bold text-xs font-mono rounded-lg transition shadow-2xs flex items-center space-x-2 shrink-0"
                >
                  <span>Buy me a SOLO ($1)</span>
                  <ExternalLink className="w-3.5 h-3.5" />
                </a>
              </div>

              {/* Legal & Financial Disclaimer */}
              <div className="p-4 bg-slate-50 border border-surface-border rounded-lg space-y-2 text-xs font-mono text-slate-600 leading-relaxed">
                <div className="flex items-center space-x-1.5 font-bold text-slate-800 text-xs">
                  <Scale className="w-3.5 h-3.5 text-slate-500" />
                  <span>Legal & Financial Risk Disclaimer</span>
                </div>
                <p>
                  This software is provided for research, simulation, and educational quantitative finance purposes. Cross-market arbitrage and algorithmic trading involve inherent execution risks, including market volatility, network latency spikes, smart contract bugs, API rate limits, and liquidity slippage. Past simulation performance does not guarantee future financial returns.
                </p>
                <p>
                  No private keys or seed phrases are stored remotely or accessed by any third party; all credentials remain encrypted locally on your system using AES-256-GCM with Argon2id key derivation. Users are solely responsible for compliance with their local jurisdiction regulations and exchange terms of service.
                </p>
              </div>
            </div>
          </div>
        )}

            </div>
          )}

      {/* Pinned Save Bar with Trading Mode Indicator on Bottom-Left */}
      <div className="p-4 border-t border-surface-border bg-slate-50/80 shrink-0 flex items-center justify-between">
        {/* Relocated Trading Mode Indicator on bottom-left */}
        <div
          className="px-2.5 py-1.5 rounded-lg border border-surface-border bg-surface text-slate-700 text-xs font-mono flex items-center space-x-1.5 shadow-2xs"
          title={`Execution mode: ${tradingMode.toUpperCase()}`}
        >
          <span className="text-muted font-normal">Mode:</span>
          <span className="font-bold text-accent-strong">{tradingMode.charAt(0).toUpperCase() + tradingMode.slice(1)}</span>
        </div>

        <div className="flex items-center space-x-2.5">
          {onClose && (
            <button
              type="button"
              onClick={onClose}
              className="px-2.5 py-2.5 bg-surface-raised hover:bg-slate-200 text-slate-700 border border-surface-border font-medium text-xs font-mono rounded-lg transition shadow-2xs cursor-pointer"
            >
              Cancel
            </button>
          )}
          <SaveButton
            type="submit"
            state={saveBtn.state}
            disabled={loading}
          />
        </div>
      </div>
    </form>
  </div>

  {/* Vault lock/unlock hoisted into the modal header (right of the title,
      before the close button). Shows on every tab — vault state is global. */}
  {headerActionsEl && api.isTauri && createPortal(
    <button
      type="button"
      onClick={isVaultLocked ? () => setShowVaultModal(true) : handleLockVault}
      className="px-2 py-1.5 rounded-lg border border-slate-200 bg-surface hover:bg-slate-100 text-slate-600 text-xs font-mono flex items-center space-x-1.5 transition shadow-2xs cursor-pointer"
      title={isVaultLocked ? "Unlock vault credentials" : "Lock sensitive exchange credentials now"}
    >
      {isVaultLocked ? (
        <>
          <Unlock className="w-3.5 h-3.5 text-warn-strong" />
          <span className="font-semibold text-slate-700">Unlock Vault</span>
        </>
      ) : (
        <>
          <Lock className="w-3.5 h-3.5 text-slate-500" />
          <span>Lock Vault</span>
        </>
      )}
    </button>,
    headerActionsEl,
  )}

  {/* Vault Unlock Modal (desktop only) */}
  <VaultUnlockModal
    isOpen={showVaultModal && api.isTauri}
    vaultStatus={vaultStatus}
    onClose={() => setShowVaultModal(false)}
    onSuccess={async (status) => {
      setVaultStatus(status);
      setShowVaultModal(false);
      try {
        const sanitized = await api.getEngineSettings();
        setConfiguredVenues(sanitized.configured_venues || []);
        if (sanitized.trading_mode === "live" || sanitized.trading_mode === "testnet" || sanitized.trading_mode === "real" || sanitized.trading_mode === "paper") {
          setTradingMode(sanitized.trading_mode);
        }
        setAccountsEpoch((prev) => prev + 1);
        api.getVenueExecutionCapabilities().then(setVenueCapabilities).catch(console.error);
        await loadVenueBalances();
      } catch (err) {
        console.warn("Failed to refresh settings after vault unlock:", err);
      }
    }}
  />

  {/* Live Trading Risk Confirmation Modal */}
  {showLiveRiskModal && (
    <div className="modal-backdrop">
      <div className="modal-card max-w-lg w-full p-6 text-slate-900 border border-surface-border rounded-2xl shadow-xl">
        <div className="flex items-center space-x-3 mb-4 pb-3 border-b border-surface-border">
          <div className="p-2.5 bg-loss-dim rounded-xl border border-loss-border text-loss shadow-2xs">
            <ShieldAlert className="w-6 h-6 text-loss" />
          </div>
          <div>
            <h3 className="text-base font-bold font-sans text-slate-900 leading-snug">
              Live Trading Risk Disclosure
            </h3>
            <p className="text-xs text-loss font-mono">Real-Capital Execution Activation</p>
          </div>
        </div>

        <div className="space-y-3.5 text-xs text-slate-600 mb-5 leading-relaxed">
          <div className="p-3 bg-loss-dim border border-loss-border rounded-xl text-loss space-y-1 font-mono text-[11px]">
            <p className="font-bold text-loss flex items-center gap-1.5">
              <AlertTriangle className="w-3.5 h-3.5 shrink-0" />
              <span>REAL CAPITAL AT RISK</span>
            </p>
            <p className="text-slate-700 leading-normal font-sans text-xs">
              You are enabling authenticated account monitoring and explicit risk-reducing close submissions. Open orders are polled for authoritative fill state at startup and every few seconds. New live strategy entries stay disabled unless the canary opt-in in Safety is enabled with a per-leg notional cap.
            </p>
          </div>

          <div className="space-y-3 text-slate-700">
            <div className="flex items-start gap-2.5">
              <span className="w-5 h-5 rounded-full bg-accent/10 text-accent-strong font-bold font-mono text-[11px] flex items-center justify-center shrink-0 mt-0.5">
                1
              </span>
              <div>
                <strong className="text-slate-900 font-semibold">Authenticated Venues Only:</strong> Close submissions are routed only to venues with authenticated API credentials in your unlocked vault. Routes involving unauthenticated or read-only venues automatically fail closed to prevent unhedged exposure.
              </div>
            </div>

            <div className="flex items-start gap-2.5">
              <span className="w-5 h-5 rounded-full bg-accent/10 text-accent-strong font-bold font-mono text-[11px] flex items-center justify-center shrink-0 mt-0.5">
                2
              </span>
              <div>
                <strong className="text-slate-900 font-semibold">Execution Latency &amp; Market Risk:</strong> Network jitter, exchange rate limits, and rapid orderbook depth fluctuations can cause slippage or partial fills. Real exchange fees and funding rates apply to all fills.
              </div>
            </div>

            <div className="flex items-start gap-2.5">
              <span className="w-5 h-5 rounded-full bg-accent/10 text-accent-strong font-bold font-mono text-[11px] flex items-center justify-center shrink-0 mt-0.5">
                3
              </span>
              <div>
                <strong className="text-slate-900 font-semibold">Safety Controls &amp; Emergency Stop:</strong> Delta guards, max grid steps, and minimum profit hurdles remain enforced. Use <span className="font-mono font-bold text-amber-700 bg-amber-50 px-1 py-0.5 rounded border border-amber-200">PAUSE</span> to freeze new submissions while preserving positions, <span className="font-mono font-bold text-loss bg-loss-dim px-1 py-0.5 rounded border border-loss-border">STOP ALL</span> to trip the circuit breaker, and explicit position close actions for risk-reducing submissions. Order acknowledgements are not proof of terminal fill; verify venue state.
              </div>
            </div>

            <div className="flex items-start gap-2.5">
              <span className="w-5 h-5 rounded-full bg-accent/10 text-accent-strong font-bold font-mono text-[11px] flex items-center justify-center shrink-0 mt-0.5">
                4
              </span>
              <div>
                <strong className="text-slate-900 font-semibold">Vault Security:</strong> API credentials remain encrypted on-device in the vault. Relocking or closing the app clears local access and stops new local submissions, but it does not cancel or prove the state of orders already accepted by a venue.
              </div>
            </div>
          </div>

          <div className="pt-3 border-t border-surface-border space-y-2.5">
            <label className="flex items-start space-x-2.5 cursor-pointer select-none">
              <input
                type="checkbox"
                checked={liveRiskAck}
                onChange={(e) => setLiveRiskAck(e.target.checked)}
                className="mt-0.5 rounded text-loss focus:ring-loss border-surface-border"
              />
              <span className="text-[11px] text-slate-700 font-medium leading-normal">
                I acknowledge the risks of real-account monitoring and explicit risk-reducing submissions, and I confirm my risk parameters and exchange credentials.
              </span>
            </label>

            <div>
              <div className="flex items-center justify-between text-[11px] font-mono text-slate-500 mb-1.5">
                <span>
                  Type <span className="text-loss font-bold select-all">CONFIRM</span> below to enable:
                </span>
                <button
                  type="button"
                  onClick={() => {
                    setLiveRiskConfirmationInput("CONFIRM");
                    setLiveRiskAck(true);
                  }}
                  className="text-[11px] text-accent-strong hover:underline font-mono cursor-pointer"
                >
                  Quick fill
                </button>
              </div>
              <input
                type="text"
                value={liveRiskConfirmationInput}
                onChange={(e) => setLiveRiskConfirmationInput(e.target.value)}
                placeholder="Type CONFIRM"
                className="input-field w-full font-mono text-xs text-slate-900"
              />
            </div>
          </div>
        </div>

        <div className="flex items-center justify-end space-x-2.5 pt-3 border-t border-surface-border">
          <button
            type="button"
            onClick={() => {
              setShowLiveRiskModal(false);
              setLiveRiskConfirmationInput("");
              setLiveRiskAck(false);
            }}
            className="btn-secondary px-2.5 py-2.5 text-xs cursor-pointer"
          >
            Cancel (Keep Testnet)
          </button>
          <button
            type="button"
            disabled={
              !liveRiskAck ||
              (liveRiskConfirmationInput.trim().toUpperCase() !== "CONFIRM" &&
               liveRiskConfirmationInput.trim().toUpperCase() !== "CONFIRM LIVE TRADING")
            }
            onClick={() => {
              const inputNorm = liveRiskConfirmationInput.trim().toUpperCase();
              if (liveRiskAck && (inputNorm === "CONFIRM" || inputNorm === "CONFIRM LIVE TRADING")) {
                setLiveRiskAgreed(true);
                setTradingMode("live");
                setShowLiveRiskModal(false);
                setLiveRiskConfirmationInput("");
                setLiveRiskAck(false);
              }
            }}
            className="btn-danger px-2.5 py-2.5 text-xs font-bold cursor-pointer disabled:opacity-40 disabled:cursor-not-allowed"
          >
            Activate Live Trading
          </button>
        </div>
      </div>
    </div>
  )}
</div>
  );
}, areEngineSettingsPropsEqual);
