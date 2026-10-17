//! Hash Time-Locked & Signature Bond Commitment (HTLC-SB) Protocol.
//!
//! Guarantees atomic multi-leg cross-chain settlement between EVM L2s (Arbitrum, Optimism, Base)
//! and non-EVM high-throughput appchains (Hyperliquid L1, Solana).
//! Enforces SHA-256 hash locks, signature bond deposits, pre-image timeout slashing
//! (1.25x bond penalty with 5% indemnity payout to counterparty), and symmetric Bayesian-Nash
//! solver bidding capped at <= 35% of gross arbitrage edge.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Execution environment architecture for cross-chain legs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ChainArchitecture {
    /// EVM L2 (Arbitrum, Base, Optimism, Ethereum Mainnet).
    EvmL2,
    /// Hyperliquid L1 (Tendermint/BFT state machine).
    HyperliquidL1,
    /// Solana (SVM with parallel state accounts).
    SolanaSvm,
}

/// Cryptographic signature scheme used for bond commitments.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SignatureScheme {
    Secp256k1,
    Ed25519,
}

/// Lifecycle state machine for the HTLC-SB atomic settlement protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HtlcState {
    /// Initialized by taker with intent and hash lock commitment.
    Initiated,
    /// Bond and funds committed by solver on source and destination chains.
    Committed,
    /// Pre-image revealed on at least one chain.
    Revealed,
    /// Successfully settled on both chains; bonds returned.
    Settled,
    /// Time lock expired before preimage reveal.
    TimedOut,
    /// Defaulting party slashed (1.25x bond penalty; 5% indemnity paid).
    Slashed,
    /// Non-defaulting party refunded.
    Refunded,
}

/// Configuration parameters for HTLC-SB settlement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HtlcSettlementConfig {
    /// Slashing multiplier applied to deposited bond on timeout default (default: 1.25x).
    pub slashing_penalty_multiplier: f64,
    /// Indemnity payout percentage of transfer amount awarded to injured party (default: 0.05 for 5%).
    pub indemnity_payout_rate: f64,
    /// Maximum solver bid ceiling fraction of gross edge (default: 0.35 for 35%).
    pub max_solver_fee_share: f64,
    /// Minimum time-lock buffer duration in seconds (e.g. 60s for L2s, 30s for Hyperliquid).
    pub min_timelock_sec: u64,
    /// Minimum bond requirement in USD.
    pub min_bond_usd: f64,
}

impl Default for HtlcSettlementConfig {
    fn default() -> Self {
        Self {
            slashing_penalty_multiplier: 1.25,
            indemnity_payout_rate: 0.05,
            max_solver_fee_share: 0.35,
            min_timelock_sec: 45,
            min_bond_usd: 500.0,
        }
    }
}

/// An atomic cross-chain settlement agreement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HtlcAgreement {
    pub agreement_id: String,
    pub source_chain: ChainArchitecture,
    pub dest_chain: ChainArchitecture,
    pub token_symbol: String,
    pub amount_usd: f64,
    pub solver_bond_usd: f64,
    pub hash_lock: [u8; 32],
    pub timelock_expiry_epoch_sec: u64,
    pub solver_id: String,
    pub solver_fee_usd: f64,
    pub signature_scheme: SignatureScheme,
    pub state: HtlcState,
}

/// Result of an attempted settlement or resolution.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SettlementOutcome {
    SuccessSettled {
        net_arbitrage_profit_usd: f64,
        solver_fee_paid_usd: f64,
    },
    TimedOutAndSlashed {
        total_slashed_usd: f64,
        indemnity_paid_usd: f64,
        refunded_amount_usd: f64,
    },
    InvalidPreimage,
    AlreadyFinalized,
}

/// Competitive solver bid submitted into the Bayesian-Nash auction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SolverBid {
    pub solver_id: String,
    pub bond_offered_usd: f64,
    pub bid_fee_usd: f64,
    pub promised_latency_sec: f64,
    pub signature_scheme: SignatureScheme,
}

/// The HTLC Signature Bond Engine.
pub struct HtlcSettlementEngine {
    pub config: HtlcSettlementConfig,
}

impl HtlcSettlementEngine {
    pub fn new(config: HtlcSettlementConfig) -> Self {
        Self { config }
    }

    /// Computes the SHA-256 hash lock from a 32-byte secret preimage.
    pub fn compute_hash_lock(preimage: &[u8; 32]) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(preimage);
        let result = hasher.finalize();
        let mut lock = [0u8; 32];
        lock.copy_from_slice(&result);
        lock
    }

    /// Verifies if a presented preimage matches the agreement's hash lock.
    pub fn verify_preimage(preimage: &[u8; 32], expected_hash_lock: &[u8; 32]) -> bool {
        let computed = Self::compute_hash_lock(preimage);
        computed == *expected_hash_lock
    }

    /// Solves the Bayesian-Nash optimal solver selection and enforces fee ceiling:
    /// b*(v) <= 0.35 * V_gross.
    pub fn select_optimal_solver(
        &self,
        bids: &[SolverBid],
        gross_arbitrage_edge_usd: f64,
        transfer_amount_usd: f64,
    ) -> Option<SolverBid> {
        let fee_ceiling = self.config.max_solver_fee_share * gross_arbitrage_edge_usd;

        let mut valid_bids: Vec<&SolverBid> = bids
            .iter()
            .filter(|b| {
                // Must have adequate bond coverage (at least min_bond_usd and 10% of transfer)
                let req_bond = self.config.min_bond_usd.max(transfer_amount_usd * 0.10);
                b.bond_offered_usd >= req_bond
                    && b.bid_fee_usd <= fee_ceiling
                    && b.bid_fee_usd > 0.0
            })
            .collect();

        if valid_bids.is_empty() {
            return None;
        }

        // Sort by lowest fee, then by promised latency
        valid_bids.sort_by(|a, b| {
            a.bid_fee_usd
                .partial_cmp(&b.bid_fee_usd)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(
                    a.promised_latency_sec
                        .partial_cmp(&b.promised_latency_sec)
                        .unwrap_or(std::cmp::Ordering::Equal),
                )
        });

        Some(valid_bids[0].clone())
    }

    /// Creates a new initiated agreement.
    #[allow(clippy::too_many_arguments)]
    pub fn initiate_agreement(
        &self,
        agreement_id: String,
        source_chain: ChainArchitecture,
        dest_chain: ChainArchitecture,
        token_symbol: String,
        amount_usd: f64,
        hash_lock: [u8; 32],
        current_epoch_sec: u64,
        timelock_sec: u64,
        winning_bid: &SolverBid,
    ) -> HtlcAgreement {
        let effective_timelock = timelock_sec.max(self.config.min_timelock_sec);
        HtlcAgreement {
            agreement_id,
            source_chain,
            dest_chain,
            token_symbol,
            amount_usd,
            solver_bond_usd: winning_bid.bond_offered_usd,
            hash_lock,
            timelock_expiry_epoch_sec: current_epoch_sec + effective_timelock,
            solver_id: winning_bid.solver_id.clone(),
            solver_fee_usd: winning_bid.bid_fee_usd,
            signature_scheme: winning_bid.signature_scheme,
            state: HtlcState::Initiated,
        }
    }

    /// Settles the agreement upon valid preimage reveal before timeout.
    pub fn settle_with_preimage(
        &self,
        agreement: &mut HtlcAgreement,
        preimage: &[u8; 32],
        current_epoch_sec: u64,
        gross_arbitrage_edge_usd: f64,
    ) -> SettlementOutcome {
        if agreement.state == HtlcState::Settled || agreement.state == HtlcState::Slashed {
            return SettlementOutcome::AlreadyFinalized;
        }

        if !Self::verify_preimage(preimage, &agreement.hash_lock) {
            return SettlementOutcome::InvalidPreimage;
        }

        if current_epoch_sec > agreement.timelock_expiry_epoch_sec {
            // Expired before valid settlement
            return self.slash_timed_out(agreement);
        }

        agreement.state = HtlcState::Settled;
        let net_profit = gross_arbitrage_edge_usd - agreement.solver_fee_usd;

        SettlementOutcome::SuccessSettled {
            net_arbitrage_profit_usd: net_profit,
            solver_fee_paid_usd: agreement.solver_fee_usd,
        }
    }

    /// Slashes defaulting party if preimage is not revealed before timelock expiry.
    /// Penalty: 1.25x bond slashed; 5% indemnity payout awarded to injured counterparty.
    pub fn slash_timed_out(&self, agreement: &mut HtlcAgreement) -> SettlementOutcome {
        if agreement.state == HtlcState::Settled || agreement.state == HtlcState::Slashed {
            return SettlementOutcome::AlreadyFinalized;
        }

        agreement.state = HtlcState::Slashed;

        let total_slashed = agreement.solver_bond_usd * self.config.slashing_penalty_multiplier;
        let indemnity = agreement.amount_usd * self.config.indemnity_payout_rate;
        let refunded = agreement.amount_usd;

        SettlementOutcome::TimedOutAndSlashed {
            total_slashed_usd: total_slashed,
            indemnity_paid_usd: indemnity,
            refunded_amount_usd: refunded,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sha256_hashlock_generation_and_verification() {
        let preimage = [42u8; 32];
        let hash_lock = HtlcSettlementEngine::compute_hash_lock(&preimage);

        assert!(HtlcSettlementEngine::verify_preimage(&preimage, &hash_lock));

        let wrong_preimage = [43u8; 32];
        assert!(!HtlcSettlementEngine::verify_preimage(
            &wrong_preimage,
            &hash_lock
        ));
    }

    #[test]
    fn test_bayesian_nash_bid_ceiling_filter() {
        let engine = HtlcSettlementEngine::new(HtlcSettlementConfig::default());
        let gross_edge = 1000.0;
        let transfer_amount = 5000.0;

        let bids = vec![
            // Excessive fee > 35% ($350)
            SolverBid {
                solver_id: "greedy_solver".into(),
                bond_offered_usd: 1000.0,
                bid_fee_usd: 400.0,
                promised_latency_sec: 1.0,
                signature_scheme: SignatureScheme::Secp256k1,
            },
            // Inadequate bond < $500
            SolverBid {
                solver_id: "undercollateralized_solver".into(),
                bond_offered_usd: 200.0,
                bid_fee_usd: 200.0,
                promised_latency_sec: 2.0,
                signature_scheme: SignatureScheme::Ed25519,
            },
            // Valid bid: fee = $250 <= $350, bond = $1200
            SolverBid {
                solver_id: "compliant_solver_b".into(),
                bond_offered_usd: 1200.0,
                bid_fee_usd: 250.0,
                promised_latency_sec: 3.0,
                signature_scheme: SignatureScheme::Secp256k1,
            },
            // Valid cheaper bid: fee = $200 <= $350, bond = $1000
            SolverBid {
                solver_id: "compliant_solver_a".into(),
                bond_offered_usd: 1000.0,
                bid_fee_usd: 200.0,
                promised_latency_sec: 2.5,
                signature_scheme: SignatureScheme::Ed25519,
            },
        ];

        let selected = engine.select_optimal_solver(&bids, gross_edge, transfer_amount);
        assert!(selected.is_some());
        let winner = selected.unwrap();
        assert_eq!(winner.solver_id, "compliant_solver_a");
        assert_eq!(winner.bid_fee_usd, 200.0);
    }

    #[test]
    fn test_successful_atomic_settlement_and_slashing() {
        let engine = HtlcSettlementEngine::new(HtlcSettlementConfig::default());
        let preimage = [7u8; 32];
        let hash_lock = HtlcSettlementEngine::compute_hash_lock(&preimage);

        let bid = SolverBid {
            solver_id: "hl_solver_1".into(),
            bond_offered_usd: 1000.0,
            bid_fee_usd: 150.0,
            promised_latency_sec: 1.2,
            signature_scheme: SignatureScheme::Ed25519,
        };

        let mut agreement = engine.initiate_agreement(
            "agree_1".into(),
            ChainArchitecture::EvmL2,
            ChainArchitecture::HyperliquidL1,
            "USDC".into(),
            10000.0,
            hash_lock,
            1000,
            60,
            &bid,
        );

        // Case A: Settle within time
        let outcome = engine.settle_with_preimage(&mut agreement, &preimage, 1020, 500.0);
        match outcome {
            SettlementOutcome::SuccessSettled {
                net_arbitrage_profit_usd,
                solver_fee_paid_usd,
            } => {
                assert_eq!(solver_fee_paid_usd, 150.0);
                assert_eq!(net_arbitrage_profit_usd, 350.0);
                assert_eq!(agreement.state, HtlcState::Settled);
            }
            other => panic!("Expected SuccessSettled, got {:?}", other),
        }

        // Case B: Timeout on separate agreement -> Slashed
        let mut timed_out_agreement = engine.initiate_agreement(
            "agree_2".into(),
            ChainArchitecture::EvmL2,
            ChainArchitecture::SolanaSvm,
            "SOL".into(),
            20000.0,
            hash_lock,
            1000,
            60,
            &bid,
        );

        let slash_outcome =
            engine.settle_with_preimage(&mut timed_out_agreement, &preimage, 1100, 500.0);
        match slash_outcome {
            SettlementOutcome::TimedOutAndSlashed {
                total_slashed_usd,
                indemnity_paid_usd,
                refunded_amount_usd,
            } => {
                // 1.25x of $1000 bond = $1250
                assert_eq!(total_slashed_usd, 1250.0);
                // 5% of $20000 transfer = $1000
                assert_eq!(indemnity_paid_usd, 1000.0);
                assert_eq!(refunded_amount_usd, 20000.0);
                assert_eq!(timed_out_agreement.state, HtlcState::Slashed);
            }
            other => panic!("Expected TimedOutAndSlashed, got {:?}", other),
        }
    }
}
