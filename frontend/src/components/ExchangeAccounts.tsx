import React, { useCallback, useEffect, useState } from "react";
import {
  AlertCircle,
  ArrowLeftRight,
  CheckCircle2,
  Eye,
  EyeOff,
  Plus,
  RefreshCw,
  Star,
  Trash2,
} from "lucide-react";
import { api } from "../services/api";
import type {
  AccountCredentialInput,
  AccountDto,
  StrategyAccountRouteDto,
} from "../types";
import { ToggleSwitch } from "./ToggleSwitch";

/* ------------------------------------------------------------------ */
/* Per-venue credential field specifications                            */
/* ------------------------------------------------------------------ */

type CredFieldKey = keyof AccountCredentialInput;

interface CredFieldSpec {
  key: CredFieldKey;
  label: string;
  secret?: boolean;
  numeric?: boolean;
  placeholder?: string;
  optional?: boolean;
}

export const ACCOUNT_CREDENTIAL_FIELDS: Record<string, CredFieldSpec[]> = {
  // Arcus: Ed25519 keypair generated + registered via the exchange (docs.arcus.xyz)
  arcus: [
    { key: "apiKey", label: "API Public Key (Ed25519)", placeholder: "arc_..." },
    { key: "privateKey", label: "API Private Key (Ed25519)", secret: true },
  ],
  // Hyperliquid: account/vault address + API wallet (agent) private key
  hyperliquid: [
    { key: "walletAddress", label: "Trading EOA Wallet Address", placeholder: "0x..." },
    { key: "privateKey", label: "API Wallet / Agent Private Key", secret: true, placeholder: "0x..." },
    { key: "accountId", label: "Vault Address", optional: true, placeholder: "0x..." },
  ],
  kalshi: [
    { key: "apiKey", label: "API Key ID", placeholder: "e.g. 550e8400-e29b-..." },
    { key: "apiSecret", label: "RSA Private Key", secret: true, placeholder: "-----BEGIN RSA PRIVATE KEY-----" },
  ],
  kraken_spot: [
    { key: "apiKey", label: "Spot API Key", secret: true },
    { key: "apiSecret", label: "Spot Private/Secret Key", secret: true },
  ],
  kraken_futures: [
    { key: "apiKey", label: "Futures API Key", secret: true },
    { key: "apiSecret", label: "Futures Secret Key", secret: true },
  ],
  // Lighter: api_private_keys{index} + account_index — each API key has its own index (0-254)
  lighter: [
    { key: "privateKey", label: "API Private Key", secret: true },
    { key: "accountIndex", label: "Account Index", numeric: true, placeholder: "e.g. 0" },
    { key: "apiKeyIndex", label: "API Key Index", numeric: true, placeholder: "0-254" },
  ],
  // Polymarket: L1 Polygon key signs orders; L2 key/secret/passphrase authenticate the CLOB API
  polymarket: [
    { key: "walletAddress", label: "Polygon Wallet Address", placeholder: "0x..." },
    { key: "privateKey", label: "Polygon Private Key (L1 Signing)", secret: true, placeholder: "0x..." },
    { key: "apiKey", label: "API Key", secret: true },
    { key: "apiSecret", label: "API Secret", secret: true },
    { key: "passphrase", label: "Passphrase", secret: true },
  ],
  // PopDEX: delegated agent — main account address + authorized agent private key (+ optional REST token)
  popdex: [
    { key: "walletAddress", label: "Main Account Address", placeholder: "0x..." },
    { key: "privateKey", label: "Agent Private Key (Authorized)", secret: true, placeholder: "0x..." },
    { key: "apiKey", label: "API Token", secret: true, optional: true },
  ],
  robinhood: [
    { key: "walletAddress", label: "EVM Wallet Address", placeholder: "0x..." },
    { key: "privateKey", label: "EVM Private Key", secret: true, placeholder: "0x..." },
    { key: "subaccountIndex", label: "Subaccount Index", numeric: true, optional: true },
  ],
  vanta: [
    { key: "accountId", label: "Account ID", placeholder: "Orderly account id" },
    { key: "apiKey", label: "Orderly Key", secret: true, placeholder: "ed25519:..." },
    { key: "apiSecret", label: "Orderly Secret", secret: true },
    { key: "subaccountIndex", label: "Subaccount Index", numeric: true, optional: true },
  ],
  // Decibel (Aptos): Geomi client key for reads, node key for SDK access, API wallet key signs txs
  decibel: [
    { key: "walletAddress", label: "Login Wallet Address (Aptos)", placeholder: "0x..." },
    { key: "privateKey", label: "API Wallet Private Key", secret: true, placeholder: "0x..." },
    { key: "apiKey", label: "Client API Key (Geomi)", secret: true },
    { key: "passphrase", label: "Node API Key (Geomi)", secret: true, optional: true },
  ],
  // Interactive Brokers: API connects via the TWS/IB Gateway session — account ID is the only stored credential
  ibkr: [
    { key: "accountId", label: "Account ID", placeholder: "e.g. U1234567 / DU1234567" },
  ],
  // IG Trading: API key header + identifier/password session login
  ig: [
    { key: "apiKey", label: "API Key", secret: true },
    { key: "accountId", label: "Username / Login ID" },
    { key: "passphrase", label: "Password", secret: true },
  ],
  binance: [
    { key: "apiKey", label: "API Key", secret: true },
    { key: "apiSecret", label: "API Secret", secret: true },
  ],
  okx: [
    { key: "apiKey", label: "API Key", secret: true },
    { key: "apiSecret", label: "API Secret", secret: true },
    { key: "passphrase", label: "Passphrase", secret: true },
  ],
  // Aster: V3 = user wallet + API wallet (agent) signer; V1 legacy key/secret still works for existing keys
  aster: [
    { key: "walletAddress", label: "Main Account Wallet (User)", placeholder: "0x..." },
    { key: "privateKey", label: "API Wallet / Agent Private Key", secret: true, placeholder: "0x..." },
    { key: "apiKey", label: "V1 API Key", secret: true, optional: true },
    { key: "apiSecret", label: "V1 API Secret", secret: true, optional: true },
  ],
  // Variational: public API is read-only today; trading auth is in development
  variational: [
    { key: "apiKey", label: "API Token", secret: true, optional: true },
  ],
  // Coinbase Advanced Trade (CDP): API key name + EC private key (PEM)
  coinbase: [
    { key: "apiKey", label: "API Key Name", placeholder: "organizations/{org}/apiKeys/{keyId}" },
    { key: "apiSecret", label: "Private Key (EC PEM)", secret: true, placeholder: "-----BEGIN EC PRIVATE KEY-----" },
  ],
};

export const ACCOUNT_VENUE_DISPLAY: Record<string, string> = {
  arcus: "Arcus",
  hyperliquid: "Hyperliquid",
  kalshi: "Kalshi",
  kraken_spot: "Kraken Spot",
  kraken_futures: "Kraken Futures",
  lighter: "Lighter",
  polymarket: "Polymarket",
  popdex: "PopDEX",
  robinhood: "Robinhood Lighter",
  vanta: "Vanta",
  decibel: "Decibel",
  ibkr: "Interactive Brokers",
  ig: "IG Trading",
  binance: "Binance",
  okx: "OKX",
  aster: "Aster",
  variational: "Variational",
  coinbase: "Coinbase",
};

const venueDisplay = (v: string) => ACCOUNT_VENUE_DISPLAY[v] ?? v;

const fieldIsSet = (a: AccountDto, key: CredFieldKey): boolean => {
  switch (key) {
    case "apiKey":
      return a.hasApiKey;
    case "apiSecret":
      return a.hasApiSecret;
    case "passphrase":
      return a.hasPassphrase;
    case "accountId":
      return a.hasAccountId;
    case "walletAddress":
      return a.hasWalletAddress;
    case "privateKey":
      return a.hasPrivateKey;
    case "accountIndex":
      return a.accountIndex != null;
    case "subaccountIndex":
      return a.subaccountIndex != null;
    case "apiKeyIndex":
      return a.apiKeyIndex != null;
    default:
      return false;
  }
};

const STORED_MASK = "••••••••••••";

/* ------------------------------------------------------------------ */
/* Credential field grid (shared by account card + add-account form)    */
/* ------------------------------------------------------------------ */

interface CredGridProps {
  fields: CredFieldSpec[];
  values: Record<string, string>;
  setValue: (key: string, value: string) => void;
  disabled: boolean;
  account?: AccountDto | null;
  showSecrets: Record<string, boolean>;
  toggleShow: (key: string) => void;
}

const CredentialGrid: React.FC<CredGridProps> = ({
  fields,
  values,
  setValue,
  disabled,
  account,
  showSecrets,
  toggleShow,
}) => (
  <div className="grid grid-cols-1 md:grid-cols-2 gap-3 text-xs font-mono">
    {fields.map((f) => {
      const stored = account ? fieldIsSet(account, f.key) : false;
      const placeholder =
        stored && f.secret
          ? STORED_MASK
          : stored && !f.secret
          ? "(stored — leave blank to keep)"
          : f.placeholder ?? (f.optional ? "Optional" : "");
      const shown = showSecrets[f.key];
      return (
        <div key={f.key}>
          <div className="flex items-center justify-between mb-1">
            <label className="text-slate-600">
              {f.label}
              {f.optional ? " (Optional)" : ""}:
            </label>
            {f.secret && (
              <button
                type="button"
                onClick={() => toggleShow(f.key)}
                className="text-slate-400 hover:text-slate-600 text-[11px] flex items-center space-x-1"
              >
                {shown ? <EyeOff className="w-3 h-3" /> : <Eye className="w-3 h-3 text-blue-600" />}
                <span>{shown ? "Hide" : "Show"}</span>
              </button>
            )}
          </div>
          <input
            type={f.secret && !shown ? "password" : f.numeric ? "number" : "text"}
            disabled={disabled}
            value={values[f.key] ?? ""}
            placeholder={placeholder}
            onChange={(e) => setValue(f.key, e.target.value)}
            autoComplete="off"
            className="w-full px-3 py-2 bg-surface-raised border border-surface-border rounded-lg text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent font-mono disabled:opacity-50"
          />
        </div>
      );
    })}
  </div>
);

/* ------------------------------------------------------------------ */
/* Account card                                                        */
/* ------------------------------------------------------------------ */

interface AccountCardProps {
  account: AccountDto;
  /** 1-based position of this account within the venue, used for the title. */
  ordinal: number;
  locked: boolean;
  /** When true the zone header owns this account's label — the in-card field is hidden. */
  labelInHeader?: boolean;
  onChanged: () => void;
}

const AccountCard: React.FC<AccountCardProps> = ({ account, ordinal, locked, labelInHeader = false, onChanged }) => {
  const fields = ACCOUNT_CREDENTIAL_FIELDS[account.venue] ?? [
    { key: "apiKey" as CredFieldKey, label: "API Key", secret: true },
    { key: "apiSecret" as CredFieldKey, label: "API Secret", secret: true },
  ];
  const [label, setLabel] = useState(account.label);
  const [drafts, setDrafts] = useState<Record<string, string>>({});
  const [showSecrets, setShowSecrets] = useState<Record<string, boolean>>({});
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);

  // Reset local drafts when the account identity changes (e.g. venue switch).
  useEffect(() => {
    setLabel(account.label);
    setDrafts({});
    setError(null);
    setSaved(false);
    setShowSecrets({});
  }, [account.id, account.label]);

  const valueFor = (key: CredFieldKey): string => {
    if (drafts[key] !== undefined) return drafts[key];
    // Numeric index fields are exposed non-secretly; prefill them so a
    // credentials save round-trips the current value instead of clearing it.
    if (key === "accountIndex") return account.accountIndex?.toString() ?? "";
    if (key === "subaccountIndex") return account.subaccountIndex?.toString() ?? "";
    if (key === "apiKeyIndex") return account.apiKeyIndex?.toString() ?? "";
    return "";
  };

  const setValue = (key: string, v: string) => setDrafts((d) => ({ ...d, [key]: v }));

  const buildCredentials = (): { creds?: AccountCredentialInput; error?: string } => {
    const creds: AccountCredentialInput = {};
    let any = false;
    for (const f of fields) {
      const raw = (valueFor(f.key) ?? "").trim();
      if (f.numeric) {
        // Index fields always serialize: non-empty sets, empty clears.
        if (raw === "") {
          (creds as Record<string, unknown>)[f.key] = null;
          any = true;
        } else {
          const n = Number.parseInt(raw, 10);
          if (!Number.isFinite(n) || n < 0) return { error: `${f.label} must be a non-negative integer` };
          (creds as Record<string, unknown>)[f.key] = n;
          any = true;
        }
      } else if (raw !== "") {
        (creds as Record<string, unknown>)[f.key] = raw;
        any = true;
      }
    }
    return { creds: any ? creds : undefined };
  };

  const run = async (fn: () => Promise<void>) => {
    setBusy(true);
    setError(null);
    setSaved(false);
    try {
      await fn();
      onChanged();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const handleSave = () =>
    run(async () => {
      const { creds, error } = buildCredentials();
      if (error) throw new Error(error);
      if (!label.trim()) throw new Error("Account label is required");
      await api.upsertAccount({
        id: account.id,
        label: label.trim(),
        enabled: account.enabled,
        makeDefault: account.isDefault,
        credentials: creds ?? null,
      });
      setDrafts({});
      setSaved(true);
    });

  const handleToggleEnabled = () =>
    run(async () => {
      await api.upsertAccount({
        id: account.id,
        label: account.label,
        enabled: !account.enabled,
        makeDefault: account.isDefault,
      });
    });

  const handleSetDefault = () =>
    run(async () => {
      await api.setDefaultAccount(account.venue, account.id);
    });

  const handleDelete = () => {
    if (!window.confirm(`Delete account "${account.label}" on ${venueDisplay(account.venue)}? Its stored credentials are removed permanently.`)) {
      return;
    }
    void run(async () => {
      await api.deleteAccount(account.id);
    });
  };

  const dirty = label.trim() !== account.label || Object.keys(drafts).length > 0;

  return (
    <div className="border border-surface-border rounded-lg bg-surface overflow-hidden">
      {/* Card header: account title + actions */}
      <div className="flex items-center justify-between gap-3 px-3.5 py-2.5 bg-slate-50/70 border-b border-surface-border">
        <div className="flex items-center gap-2 min-w-0 flex-1">
          <span className="text-xs font-mono font-bold text-slate-900 truncate">
            {venueDisplay(account.venue)} Account {ordinal}
          </span>
          {account.isDefault && (
            <span className="px-1.5 py-0.5 rounded-sm text-[9px] font-mono font-bold bg-amber-50 text-amber-700 border border-amber-200 uppercase tracking-wide shrink-0">
              Default
            </span>
          )}
        </div>
        <div className="flex items-center gap-2.5 shrink-0">
          {!account.isDefault && (
            <button
              type="button"
              disabled={busy || !account.enabled}
              onClick={() => void handleSetDefault()}
              className="text-slate-400 hover:text-amber-500 disabled:opacity-40 transition cursor-pointer"
              title="Make this the default account for the venue"
            >
              <Star className="w-3.5 h-3.5" />
            </button>
          )}
          <ToggleSwitch
            size="sm"
            checked={account.enabled}
            onChange={() => void handleToggleEnabled()}
            disabled={busy}
            label={account.enabled ? "Enabled" : "Off"}
          />
          <button
            type="button"
            disabled={busy}
            onClick={handleDelete}
            className="text-slate-400 hover:text-red-500 disabled:opacity-40 transition cursor-pointer"
            title="Delete this account and its stored credentials"
          >
            <Trash2 className="w-3.5 h-3.5" />
          </button>
        </div>
      </div>

      {/* Card body: label + credentials */}
      <div className="p-3.5 space-y-3">
        {!labelInHeader && (
          <div className="text-xs font-mono">
            <label className="text-slate-600 block mb-1">
              Account Label <span className="text-slate-400">(used in strategy routing)</span>:
            </label>
            <input
              type="text"
              value={label}
              disabled={busy}
              onChange={(e) => setLabel(e.target.value)}
              placeholder={`${venueDisplay(account.venue)} Account ${ordinal}`}
              className="w-full max-w-56 px-2 py-1.5 bg-surface-raised border border-surface-border rounded-md text-xs font-mono text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent"
            />
          </div>
        )}
        <CredentialGrid
          fields={fields}
          values={Object.fromEntries(fields.map((f) => [f.key, valueFor(f.key)]))}
          setValue={setValue}
          disabled={busy || locked}
          account={account}
          showSecrets={showSecrets}
          toggleShow={(k) => setShowSecrets((s) => ({ ...s, [k]: !s[k] }))}
        />

        <div className="flex items-center justify-between pt-1">
          <div className="text-[10px] font-mono text-slate-400">
            {account.credentialsConfigured === null
              ? "Unlock the vault to manage credentials"
              : account.credentialsConfigured
              ? "Credentials stored in encrypted vault"
              : "No credentials stored for this account"}
          </div>
          <button
            type="button"
            disabled={busy || locked || !dirty}
            onClick={() => void handleSave()}
            className="px-3 py-1.5 bg-accent hover:bg-blue-600 text-white text-[11px] font-mono font-bold rounded-md transition shadow-2xs disabled:opacity-40 cursor-pointer flex items-center gap-1.5"
          >
            {saved && !dirty ? <CheckCircle2 className="w-3 h-3" /> : null}
            <span>{busy ? "Saving..." : saved && !dirty ? "Saved" : "Save Account"}</span>
          </button>
        </div>

        {error && (
          <div className="p-2 bg-red-50 border border-red-200 rounded-md text-[11px] font-mono text-red-600 flex items-start gap-1.5">
            <AlertCircle className="w-3.5 h-3.5 shrink-0 mt-0.5" />
            <span>{error}</span>
          </div>
        )}
      </div>
    </div>
  );
};

/* ------------------------------------------------------------------ */
/* Add-account form                                                    */
/* ------------------------------------------------------------------ */

/** Label drafts keyed by venue — the zone-header label input writes here so a not-yet-created account form can read it at submit time. */
const accountLabelDrafts = new Map<string, string>();

interface AddAccountFormProps {
  venue: string;
  hasDefault: boolean;
  locked: boolean;
  /** When true the account label is owned by the zone header input — the form omits its own label field. */
  labelFromHeader?: boolean;
  onDone: () => void;
  onCancel?: () => void;
}

const AddAccountForm: React.FC<AddAccountFormProps> = ({ venue, hasDefault, locked, labelFromHeader = false, onDone, onCancel }) => {
  const fields = ACCOUNT_CREDENTIAL_FIELDS[venue] ?? [
    { key: "apiKey" as CredFieldKey, label: "API Key", secret: true },
    { key: "apiSecret" as CredFieldKey, label: "API Secret", secret: true },
  ];
  const [label, setLabel] = useState("");
  const [drafts, setDrafts] = useState<Record<string, string>>({});
  const [showSecrets, setShowSecrets] = useState<Record<string, boolean>>({});
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const handleCreate = async () => {
    setBusy(true);
    setError(null);
    try {
      const effectiveLabel = labelFromHeader
        ? (accountLabelDrafts.get(venue)?.trim() || `${venueDisplay(venue)} #1`)
        : label.trim();
      if (!effectiveLabel) throw new Error("Account label is required");
      const creds: AccountCredentialInput = {};
      let any = false;
      for (const f of fields) {
        const raw = (drafts[f.key] ?? "").trim();
        if (raw === "") continue;
        if (f.numeric) {
          const n = Number.parseInt(raw, 10);
          if (!Number.isFinite(n) || n < 0) throw new Error(`${f.label} must be a non-negative integer`);
          (creds as Record<string, unknown>)[f.key] = n;
        } else {
          (creds as Record<string, unknown>)[f.key] = raw;
        }
        any = true;
      }
      await api.upsertAccount({
        venue,
        label: effectiveLabel,
        enabled: true,
        makeDefault: !hasDefault,
        credentials: any ? creds : null,
      });
      onDone();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      setBusy(false);
    }
  };

  return (
    <div className="border border-accent-line/60 rounded-lg bg-blue-50/30 overflow-hidden">
      <div className="px-3.5 py-2.5 bg-blue-50/70 border-b border-accent-line/40 flex items-center justify-between">
        <span className="text-[11px] font-mono font-bold text-slate-800 tracking-wide">
          Account Credential
        </span>
        {onCancel && (
          <button
            type="button"
            onClick={onCancel}
            disabled={busy}
            className="text-slate-400 hover:text-slate-600 text-[11px] font-mono cursor-pointer"
          >
            Cancel
          </button>
        )}
      </div>
      <div className="p-3.5 space-y-3">
        {!labelFromHeader && (
          <div className="text-xs font-mono">
            <label className="text-slate-600 block mb-1">Account Label:</label>
            <input
              type="text"
              value={label}
              disabled={busy}
              placeholder="e.g. Main, Market-Making, Arbitrage-2"
              onChange={(e) => setLabel(e.target.value)}
              autoFocus
              className="w-full px-3 py-2 bg-surface-raised border border-surface-border rounded-lg text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent font-mono"
            />
          </div>
        )}

        <CredentialGrid
          fields={fields}
          values={drafts}
          setValue={(k, v) => setDrafts((d) => ({ ...d, [k]: v }))}
          disabled={busy || locked}
          account={null}
          showSecrets={showSecrets}
          toggleShow={(k) => setShowSecrets((s) => ({ ...s, [k]: !s[k] }))}
        />
        {locked && (
          <p className="text-[10px] font-mono text-amber-600">
            Vault is locked: the account can be created now, but credentials require an unlocked vault.
          </p>
        )}

        {error && (
          <div className="p-2 bg-red-50 border border-red-200 rounded-md text-[11px] font-mono text-red-600 flex items-start gap-1.5">
            <AlertCircle className="w-3.5 h-3.5 shrink-0 mt-0.5" />
            <span>{error}</span>
          </div>
        )}

        <div className="flex justify-end">
          <button
            type="button"
            disabled={busy || (!labelFromHeader && !label.trim())}
            onClick={() => void handleCreate()}
            className="px-3.5 py-1.5 bg-surface-raised border border-surface-border hover:bg-slate-200 text-slate-700 text-[11px] font-mono font-semibold rounded-md transition disabled:opacity-40 cursor-pointer"
          >
            {busy ? "Saving..." : "Save"}
          </button>
        </div>
      </div>
    </div>
  );
};

/* ------------------------------------------------------------------ */
/* Venue accounts section (balance + account cards)                    */
/* ------------------------------------------------------------------ */

interface VenueAccountsSectionProps {
  venue: string;
  locked: boolean;
  compact?: boolean;
  /** Bump to force a reload (e.g. after an account is created elsewhere). */
  refreshKey?: number;
  /** Notifies the parent when account data mutates (so sibling controls like the zone header can refresh). */
  onChanged?: () => void;
}

export const VenueAccountsSection: React.FC<VenueAccountsSectionProps> = ({
  venue,
  locked,
  compact = false,
  refreshKey = 0,
  onChanged,
}) => {
  const [accounts, setAccounts] = useState<AccountDto[] | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);

  const reload = useCallback(async () => {
    try {
      setAccounts(await api.listAccounts(venue));
      setLoadError(null);
    } catch (e) {
      setLoadError(e instanceof Error ? e.message : String(e));
    }
  }, [venue]);

  useEffect(() => {
    void reload();
  }, [reload, refreshKey]);

  return (
    <div className={compact ? "space-y-3" : "space-y-3 border-t border-surface-border/60 pt-3"}>


      {loadError && (
        <div className="p-2 bg-red-50 border border-red-200 rounded-md text-[11px] font-mono text-red-600">
          {loadError}
        </div>
      )}

      {accounts && accounts.length === 0 && (
        <AddAccountForm
          venue={venue}
          hasDefault={false}
          locked={locked}
          labelFromHeader
          onDone={() => {
            accountLabelDrafts.delete(venue);
            void reload();
            onChanged?.();
          }}
        />
      )}

      <div className="space-y-2.5">
        {(accounts ?? []).map((a, i) => (
          <AccountCard key={a.id} account={a} ordinal={i + 1} locked={locked} labelInHeader={i === 0} onChanged={() => { void reload(); onChanged?.(); }} />
        ))}
      </div>
    </div>
  );
};

/* ------------------------------------------------------------------ */
/* Add-account slot — dashed placeholder that expands into the form    */
/* ------------------------------------------------------------------ */

/** Renders below an account zone: a dashed "+ Add another account" gadget that expands into the new-account form in place. */
export const AddAccountSlot: React.FC<{
  venue: string;
  locked: boolean;
  onChanged?: () => void;
}> = ({ venue, locked, onChanged }) => {
  const [adding, setAdding] = useState(false);
  const [accounts, setAccounts] = useState<AccountDto[] | null>(null);

  useEffect(() => {
    api
      .listAccounts(venue)
      .then(setAccounts)
      .catch(() => setAccounts(null));
  }, [venue]);

  if (adding) {
    return (
      <AddAccountForm
        venue={venue}
        hasDefault={(accounts ?? []).some((a) => a.isDefault)}
        locked={locked}
        onDone={() => {
          setAdding(false);
          onChanged?.();
        }}
        onCancel={() => setAdding(false)}
      />
    );
  }

  return (
    <button
      type="button"
      onClick={() => setAdding(true)}
      disabled={locked}
      className="w-full min-h-[118px] border border-dashed border-surface-border rounded-xl bg-transparent hover:bg-surface-raised/60 hover:border-accent-line flex flex-col items-center justify-center gap-1.5 text-slate-400 hover:text-accent transition cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
    >
      <Plus className="w-5 h-5" />
      <span className="text-[11px] font-mono font-semibold">Add Another Account</span>
    </button>
  );
};

/* ------------------------------------------------------------------ */
/* Account label control — sits in the zone header, edits account #1   */
/* ------------------------------------------------------------------ */

/** Editable label for the zone's account. When the zone has no account yet, the input holds the draft the inline credential form submits. */
export const AccountLabelControl: React.FC<{
  venue: string;
  refreshKey?: number;
  onChanged?: () => void;
}> = ({ venue, refreshKey = 0, onChanged }) => {
  const [account, setAccount] = useState<AccountDto | null | undefined>(undefined);
  const [draft, setDraft] = useState(() => accountLabelDrafts.get(venue) ?? "");
  const [editing, setEditing] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    api
      .listAccounts(venue)
      .then((list) => setAccount(list[0] ?? null))
      .catch(() => setAccount(null));
  }, [venue, refreshKey]);

  const saveLabel = (value: string) => {
    setEditing(null);
    const next = value.trim();
    if (!account || !next || next === account.label) return;
    setBusy(true);
    void api
      .upsertAccount({ id: account.id, label: next, enabled: account.enabled, makeDefault: account.isDefault })
      .then(() => onChanged?.())
      .catch(() => undefined)
      .finally(() => setBusy(false));
  };

  if (account === undefined) return null;

  return (
    <input
      type="text"
      value={account ? (editing ?? account.label) : draft}
      disabled={busy}
      placeholder={`${venueDisplay(venue)} #1`}
      title="Account Label"
      onChange={(e) => {
        if (account) {
          setEditing(e.target.value);
        } else {
          setDraft(e.target.value);
          accountLabelDrafts.set(venue, e.target.value);
        }
      }}
      onBlur={(e) => account && saveLabel(e.target.value)}
      onKeyDown={(e) => {
        if (e.key === "Enter") account && saveLabel((e.target as HTMLInputElement).value);
        if (e.key === "Escape") setEditing(null);
      }}
      className="w-40 px-2 py-0.5 bg-surface-raised border border-surface-border rounded-md text-[10px] font-mono text-slate-900 placeholder:text-slate-400 focus:outline-none focus:ring-1 focus:ring-accent disabled:opacity-50"
    />
  );
};

/* ------------------------------------------------------------------ */
/* Venue default-account control — sits left of a zone toggle          */
/* ------------------------------------------------------------------ */

/** The account zone's first account can be promoted to the venue default. Renders a badge once set, nothing when the zone has no account. */
export const VenueDefaultControl: React.FC<{
  venue: string;
  refreshKey?: number;
  onChanged?: () => void;
}> = ({ venue, refreshKey = 0, onChanged }) => {
  const [account, setAccount] = useState<AccountDto | null>(null);
  const [busy, setBusy] = useState(false);

  const reload = useCallback(async () => {
    try {
      const list = await api.listAccounts(venue);
      setAccount(list[0] ?? null);
    } catch {
      setAccount(null);
    }
  }, [venue]);

  useEffect(() => {
    void reload();
  }, [reload, refreshKey]);

  const disabled = busy || !account || account.isDefault;

  return (
    <button
      type="button"
      disabled={disabled}
      onClick={() => {
        if (!account) return;
        setBusy(true);
        void api
          .setDefaultAccount(venue, account.id)
          .then(() => {
            void reload();
            onChanged?.();
          })
          .finally(() => setBusy(false));
      }}
      className="px-3 py-1 rounded-md border border-surface-border text-[11px] font-mono font-semibold text-slate-500 hover:text-amber-600 hover:border-amber-300 transition cursor-pointer disabled:opacity-40 disabled:cursor-not-allowed disabled:hover:text-slate-500 disabled:hover:border-surface-border"
      title={
        !account
          ? "No account configured for this venue"
          : account.isDefault
            ? "This account is the venue default"
            : "Set this account as the venue default"
      }
    >
      Set As Default
    </button>
  );
};

/* ------------------------------------------------------------------ */
/* Strategy-level account routing                                      */
/* ------------------------------------------------------------------ */

export const STRATEGIES: { id: string; name: string; desc: string }[] = [
  { id: "futures", name: "Futures / Spatial Arbitrage", desc: "Same-ticker price dislocations across perp venues" },
  { id: "funding", name: "Funding Rate Carry", desc: "Long/short perp pairs capturing funding differentials" },
  { id: "spot", name: "Spot Cross-Exchange", desc: "Spot basis vs. perp or spot venues" },
  { id: "prediction", name: "Prediction Market Parity", desc: "Binary market and cross-venue parity trades" },
  { id: "stat_arb", name: "Statistical Arbitrage", desc: "Mean-reversion and basket spread strategies" },
];

interface RouteLegPickerProps {
  accounts: AccountDto[];
  venue: string;
  accountId: string;
  onVenue: (v: string) => void;
  onAccount: (id: string) => void;
  otherVenue?: string;
  disabled: boolean;
}

const RouteLegPicker: React.FC<RouteLegPickerProps> = ({
  accounts,
  venue,
  accountId,
  onVenue,
  onAccount,
  otherVenue,
  disabled,
}) => {
  const venueAccounts = accounts.filter((a) => a.venue === venue && a.enabled);
  // Only venues with at least one enabled account are routable
  const routableVenues = Object.keys(ACCOUNT_VENUE_DISPLAY).filter((v) =>
    accounts.some((a) => a.venue === v && a.enabled)
  );
  // Keep a currently-selected venue visible even if its accounts were since disabled
  const venueOptions =
    venue && !routableVenues.includes(venue) ? [...routableVenues, venue] : routableVenues;
  const selectClass =
    "w-full px-2.5 py-1.5 bg-surface border border-surface-border rounded-md text-[11px] font-mono text-slate-800 focus:outline-none focus:ring-1 focus:ring-accent disabled:opacity-50 cursor-pointer";
  return (
    <div className="flex-1 min-w-0 grid grid-cols-2 gap-2">
      <select
        value={venue}
        disabled={disabled}
        onChange={(e) => onVenue(e.target.value)}
        className={selectClass}
      >
        <option value="">
          {routableVenues.length === 0 ? "No enabled accounts" : "Exchange..."}
        </option>
        {venueOptions
          .filter((v) => v !== otherVenue)
          .map((v) => (
            <option key={v} value={v}>
              {venueDisplay(v)}
            </option>
          ))}
      </select>
      <select
        value={accountId}
        disabled={disabled || !venue}
        onChange={(e) => onAccount(e.target.value)}
        className={selectClass}
      >
        <option value="">
          {venue
            ? venueAccounts.length === 0
              ? "No enabled accounts"
              : "Account..."
            : "Account..."}
        </option>
        {venueAccounts.map((a) => (
          <option key={a.id} value={a.id}>
            {a.label}
            {a.isDefault ? " (default)" : ""}
          </option>
        ))}
      </select>
    </div>
  );
};

interface StrategyRouteCardProps {
  strategy: { id: string; name: string; desc: string };
  routes: StrategyAccountRouteDto[];
  accounts: AccountDto[];
  onChanged: () => void;
}

const StrategyRouteCard: React.FC<StrategyRouteCardProps> = ({ strategy, routes, accounts, onChanged }) => {
  const [adding, setAdding] = useState(false);
  const [venueA, setVenueA] = useState("");
  const [acctA, setAcctA] = useState("");
  const [venueB, setVenueB] = useState("");
  const [acctB, setAcctB] = useState("");
  const [routeLabel, setRouteLabel] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const resetForm = () => {
    setAdding(false);
    setVenueA("");
    setAcctA("");
    setVenueB("");
    setAcctB("");
    setRouteLabel("");
    setError(null);
  };

  const handleAdd = async () => {
    setBusy(true);
    setError(null);
    try {
      if (!acctA || !acctB) throw new Error("Select an account on each exchange");
      await api.upsertStrategyRoute({
        strategyId: strategy.id,
        accountAId: acctA,
        accountBId: acctB,
        label: routeLabel.trim() || undefined,
        enabled: true,
      });
      resetForm();
      onChanged();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const handleToggle = async (r: StrategyAccountRouteDto) => {
    setBusy(true);
    setError(null);
    try {
      await api.upsertStrategyRoute({ id: r.id, strategyId: r.strategyId, accountAId: r.accountAId, accountBId: r.accountBId, enabled: !r.enabled });
      onChanged();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const handleDelete = async (r: StrategyAccountRouteDto) => {
    if (!window.confirm(`Delete route "${r.label}" for ${strategy.name}?`)) return;
    setBusy(true);
    setError(null);
    try {
      await api.deleteStrategyRoute(r.id);
      onChanged();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const accountName = (id: string, fallback: string) =>
    accounts.find((a) => a.id === id)?.label ?? fallback;

  return (
    <div className="bg-surface border border-surface-border rounded-xl p-4 space-y-3 shadow-xs">
      <div className="flex items-center justify-between border-b border-surface-border pb-2.5">
        <div>
          <h4 className="text-xs font-mono font-bold text-slate-900">{strategy.name}</h4>
          <p className="text-[10px] font-mono text-slate-500 mt-0.5">{strategy.desc}</p>
        </div>
        <button
          type="button"
          onClick={() => setAdding((v) => !v)}
          className="px-2 py-1 rounded-md border border-accent-line/60 bg-blue-50/60 hover:bg-blue-100/70 text-accent text-[10px] font-mono font-bold flex items-center gap-1 transition cursor-pointer shrink-0"
        >
          <Plus className="w-3 h-3" />
          <span>Add Route</span>
        </button>
      </div>

      {routes.length === 0 && !adding && (
        <div className="p-3 bg-surface-raised border border-dashed border-surface-border rounded-lg text-[11px] font-mono text-slate-500">
          No account routes. Every ticker using this strategy falls back to each exchange&apos;s
          <span className="font-bold"> default account</span>.
        </div>
      )}

      {routes.map((r) => (
        <div
          key={r.id}
          className="flex items-center justify-between gap-3 px-3 py-2 bg-surface-raised border border-surface-border rounded-lg"
        >
          <div className="flex items-center gap-2 min-w-0 text-[11px] font-mono">
            <span className="font-bold text-slate-800 truncate">{r.label}</span>
            <span className="text-slate-300">|</span>
            <span className="text-slate-600 truncate">
              {venueDisplay(r.accountAVenue)}:{accountName(r.accountAId, r.accountALabel)}
            </span>
            <ArrowLeftRight className="w-3 h-3 text-slate-400 shrink-0" />
            <span className="text-slate-600 truncate">
              {venueDisplay(r.accountBVenue)}:{accountName(r.accountBId, r.accountBLabel)}
            </span>
          </div>
          <div className="flex items-center gap-2 shrink-0">
            <ToggleSwitch
              size="sm"
              checked={r.enabled}
              disabled={busy}
              onChange={() => void handleToggle(r)}
              label={r.enabled ? "On" : "Off"}
            />
            <button
              type="button"
              disabled={busy}
              onClick={() => void handleDelete(r)}
              className="text-slate-400 hover:text-red-500 disabled:opacity-40 transition cursor-pointer"
              title="Delete route"
            >
              <Trash2 className="w-3.5 h-3.5" />
            </button>
          </div>
        </div>
      ))}

      {adding && (
        <div className="p-3 bg-blue-50/40 border border-accent-line/50 rounded-lg space-y-2.5">
          <div className="flex items-center gap-2">
            <RouteLegPicker
              accounts={accounts}
              venue={venueA}
              accountId={acctA}
              otherVenue={venueB}
              disabled={busy}
              onVenue={(v) => {
                setVenueA(v);
                setAcctA("");
              }}
              onAccount={setAcctA}
            />
            <ArrowLeftRight className="w-3.5 h-3.5 text-slate-400 shrink-0" />
            <RouteLegPicker
              accounts={accounts}
              venue={venueB}
              accountId={acctB}
              otherVenue={venueA}
              disabled={busy}
              onVenue={(v) => {
                setVenueB(v);
                setAcctB("");
              }}
              onAccount={setAcctB}
            />
          </div>
          <input
            type="text"
            value={routeLabel}
            disabled={busy}
            placeholder="Route label (optional)"
            onChange={(e) => setRouteLabel(e.target.value)}
            className="w-full px-2.5 py-1.5 bg-surface border border-surface-border rounded-md text-[11px] font-mono text-slate-800 focus:outline-none focus:ring-1 focus:ring-accent"
          />
          {error && (
            <div className="text-[10px] font-mono text-red-600 flex items-center gap-1">
              <AlertCircle className="w-3 h-3" />
              <span>{error}</span>
            </div>
          )}
          <div className="flex justify-end gap-2">
            <button
              type="button"
              onClick={resetForm}
              disabled={busy}
              className="px-2.5 py-1 text-[10px] font-mono text-slate-500 hover:text-slate-700 cursor-pointer"
            >
              Cancel
            </button>
            <button
              type="button"
              disabled={busy || !acctA || !acctB}
              onClick={() => void handleAdd()}
              className="px-3 py-1 bg-accent hover:bg-blue-600 text-white text-[10px] font-mono font-bold rounded-md transition disabled:opacity-40 cursor-pointer"
            >
              {busy ? "Adding..." : "Add Route"}
            </button>
          </div>
        </div>
      )}

      {error && !adding && (
        <div className="text-[10px] font-mono text-red-600 flex items-center gap-1">
          <AlertCircle className="w-3 h-3" />
          <span>{error}</span>
        </div>
      )}
    </div>
  );
};

export const StrategyRoutingPanel: React.FC = () => {
  const [routes, setRoutes] = useState<StrategyAccountRouteDto[] | null>(null);
  const [accounts, setAccounts] = useState<AccountDto[]>([]);
  const [loadError, setLoadError] = useState<string | null>(null);

  const reload = useCallback(async () => {
    try {
      const [r, a] = await Promise.all([api.listStrategyRoutes(), api.listAccounts()]);
      setRoutes(r);
      setAccounts(a);
      setLoadError(null);
    } catch (e) {
      setLoadError(e instanceof Error ? e.message : String(e));
    }
  }, []);

  useEffect(() => {
    void reload();
  }, [reload]);

  return (
    <div className="space-y-4">
      <div className="flex items-center justify-between">
        <p className="text-[11px] font-mono text-slate-500 max-w-2xl">
          Account routes bind an account on one exchange to an account on another for every ticker
          using the strategy. Tickers without a route fall back to each exchange&apos;s default account.
        </p>
        <button
          type="button"
          onClick={() => void reload()}
          className="p-1.5 text-slate-400 hover:text-slate-600 transition cursor-pointer"
          title="Refresh routes"
        >
          <RefreshCw className="w-3.5 h-3.5" />
        </button>
      </div>

      {loadError && (
        <div className="p-3 bg-red-50 border border-red-200 rounded-lg text-[11px] font-mono text-red-600">
          {loadError}
        </div>
      )}

      {STRATEGIES.map((s) => (
        <StrategyRouteCard
          key={s.id}
          strategy={s}
          routes={(routes ?? []).filter((r) => r.strategyId === s.id)}
          accounts={accounts}
          onChanged={() => void reload()}
        />
      ))}
    </div>
  );
};
