//! Across Protocol SpokePool V3 & ERC-7683 Intent-Based Fast Bridge Adapter.
//!
//! Provides on-chain deposit encoding, solver Dutch auction fee discovery,
//! and canonical cross-chain deployment addresses for EVM L2/L1 collateral transfers.

#![allow(clippy::too_many_arguments)]

use alloy::primitives::{Address, Bytes, B256, U256};
use alloy::sol;
use alloy::sol_types::{SolCall, SolEvent};
use serde::{Deserialize, Serialize};

sol! {
    #[derive(Debug, PartialEq)]
    interface IAcrossSpokePool {
        function depositV3(
            address depositor,
            address recipient,
            address inputToken,
            address outputToken,
            uint256 inputAmount,
            uint256 outputAmount,
            uint256 destinationChainId,
            address exclusiveRelayer,
            uint32 quoteTimestamp,
            uint32 fillDeadline,
            uint32 exclusivityDeadline,
            bytes calldata message
        ) external payable;

        event V3FundsDeposited(
            address inputToken,
            address outputToken,
            uint256 inputAmount,
            uint256 outputAmount,
            uint256 indexed destinationChainId,
            uint32 indexed depositId,
            uint32 quoteTimestamp,
            uint32 fillDeadline,
            uint32 exclusivityDeadline,
            address indexed depositor,
            address recipient,
            address exclusiveRelayer,
            bytes message
        );

        event FilledRelay(
            address inputToken,
            address outputToken,
            uint256 inputAmount,
            uint256 outputAmount,
            uint256 repaymentChainId,
            uint256 indexed originChainId,
            uint32 indexed depositId,
            uint32 fillDeadline,
            uint32 exclusivityDeadline,
            address exclusiveRelayer,
            address indexed relayer,
            address depositor,
            address recipient,
            bytes message
        );

        event RefundedRelay(
            address inputToken,
            uint256 inputAmount,
            uint256 indexed destinationChainId,
            uint32 indexed depositId,
            address indexed depositor
        );
    }

    interface IERC7683Settler {
        struct OnchainCrossChainOrder {
            uint32 fillDeadline;
            bytes32 orderDataType;
            bytes orderData;
        }
        function open(OnchainCrossChainOrder calldata order) external payable;
    }
}

/// Known Across Protocol SpokePool V3 canonical deployments.
pub fn spoke_pool_address(chain_id: u64) -> Option<Address> {
    match chain_id {
        1 => "0x5c7BCd6E7De5423a257D81B442095A1a6ced35C5".parse().ok(), // Ethereum L1
        42161 => "0xe35e9842fceaCA96570B734083f4a58e8F7C5f2A".parse().ok(), // Arbitrum One
        8453 => "0x09aea4b2242abC8bb4BB78D537A67a245A7bEC64".parse().ok(), // Base
        10 => "0x6f26Bf09B1C792e3228e5467807a900A50De2918".parse().ok(), // Optimism
        _ => None,
    }
}

/// Request parameters for an Across V3 intent bridge deposit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AcrossBridgeDepositRequest {
    pub origin_chain_id: u64,
    pub destination_chain_id: u64,
    pub depositor: String,
    pub recipient: String,
    pub input_token: String,
    pub output_token: String,
    pub input_amount_units: String,
    pub output_amount_units: String,
    pub quote_timestamp: u32,
    pub fill_deadline: u32,
    pub exclusive_relayer: Option<String>,
    pub exclusivity_deadline: Option<u32>,
    pub message: Option<Vec<u8>>,
}

/// Evaluated quote for an Across intent bridge transfer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AcrossBridgeQuote {
    pub input_amount_usd: f64,
    pub output_amount_usd: f64,
    pub fee_bps: f64,
    pub total_fee_usd: f64,
    pub estimated_latency_sec: f64,
}

/// Across Intent Bridge Adapter for automated collateral rebalancing.
#[derive(Debug, Clone, Default)]
pub struct AcrossIntentBridgeAdapter {
    pub solver_engine: arbitrageur_math::intent_bridge::CrossChainIntentBridgeEngine,
}

impl AcrossIntentBridgeAdapter {
    pub fn new() -> Self {
        Self {
            solver_engine: arbitrageur_math::intent_bridge::CrossChainIntentBridgeEngine::default(),
        }
    }

    /// Evaluates dynamic solver Dutch auction pricing and outputs an Across bridge quote.
    pub fn evaluate_quote(
        &self,
        input_amount_usd: f64,
        urgency_multiplier: f64,
    ) -> AcrossBridgeQuote {
        let (optimal_fee_bps, latency_sec) = self
            .solver_engine
            .compute_optimal_solver_fee(urgency_multiplier);
        let fee_pct = optimal_fee_bps / 10000.0;
        let fee_usd =
            input_amount_usd * fee_pct + self.solver_engine.config.dest_gas_reimbursement_usd;
        let output_usd = (input_amount_usd - fee_usd).max(0.0);

        AcrossBridgeQuote {
            input_amount_usd,
            output_amount_usd: output_usd,
            fee_bps: optimal_fee_bps,
            total_fee_usd: fee_usd,
            estimated_latency_sec: latency_sec,
        }
    }

    /// Encodes calldata for `IAcrossSpokePool::depositV3`.
    pub fn encode_deposit_v3(&self, req: &AcrossBridgeDepositRequest) -> Result<Vec<u8>, String> {
        if spoke_pool_address(req.origin_chain_id).is_none() {
            return Err(format!(
                "Unsupported origin chain ID {}: no canonical SpokePool deployment",
                req.origin_chain_id
            ));
        }
        if spoke_pool_address(req.destination_chain_id).is_none() {
            return Err(format!(
                "Unsupported destination chain ID {}: no canonical SpokePool deployment",
                req.destination_chain_id
            ));
        }

        let depositor: Address = req
            .depositor
            .parse()
            .map_err(|e| format!("Invalid depositor address: {e}"))?;
        let recipient: Address = req
            .recipient
            .parse()
            .map_err(|e| format!("Invalid recipient address: {e}"))?;
        let input_token: Address = req
            .input_token
            .parse()
            .map_err(|e| format!("Invalid input token address: {e}"))?;
        let output_token: Address = req
            .output_token
            .parse()
            .map_err(|e| format!("Invalid output token address: {e}"))?;

        let input_amount: U256 = req
            .input_amount_units
            .parse()
            .map_err(|e| format!("Invalid input amount U256: {e}"))?;
        let output_amount: U256 = req
            .output_amount_units
            .parse()
            .map_err(|e| format!("Invalid output amount U256: {e}"))?;

        let dest_chain_id = U256::from(req.destination_chain_id);

        let exclusive_relayer: Address = if let Some(ref relayer) = req.exclusive_relayer {
            relayer
                .parse()
                .map_err(|e| format!("Invalid exclusive relayer: {e}"))?
        } else {
            Address::ZERO
        };

        let exclusivity_deadline = req.exclusivity_deadline.unwrap_or(0);
        let message = Bytes::from(req.message.clone().unwrap_or_default());

        let call = IAcrossSpokePool::depositV3Call {
            depositor,
            recipient,
            inputToken: input_token,
            outputToken: output_token,
            inputAmount: input_amount,
            outputAmount: output_amount,
            destinationChainId: dest_chain_id,
            exclusiveRelayer: exclusive_relayer,
            quoteTimestamp: req.quote_timestamp,
            fillDeadline: req.fill_deadline,
            exclusivityDeadline: exclusivity_deadline,
            message,
        };

        Ok(call.abi_encode())
    }

    /// Encodes calldata for `IERC7683Settler::open`.
    pub fn encode_erc7683_open(
        &self,
        fill_deadline: u32,
        order_data_type: [u8; 32],
        order_data: Vec<u8>,
    ) -> Vec<u8> {
        let call = IERC7683Settler::openCall {
            order: IERC7683Settler::OnchainCrossChainOrder {
                fillDeadline: fill_deadline,
                orderDataType: order_data_type.into(),
                orderData: order_data.into(),
            },
        };

        call.abi_encode()
    }

    /// Prepares an Across V3 deposit for private builder relay submission (MEV protection).
    pub fn prepare_private_deposit_submission(
        &self,
        req: &AcrossBridgeDepositRequest,
        gross_profit_usd: f64,
        gas_units: f64,
        base_fee_gwei: f64,
        eth_price_usd: f64,
    ) -> Result<AcrossBridgePrivateSubmissionPlan, String> {
        let spoke_pool = spoke_pool_address(req.origin_chain_id).ok_or_else(|| {
            format!(
                "No SpokePool deployment known for chain {}",
                req.origin_chain_id
            )
        })?;
        let calldata = self.encode_deposit_v3(req)?;

        let relay_engine = arbitrageur_math::private_relay::PrivateBuilderMevRelayEngine::default();
        let gas_rec = relay_engine.compute_priority_fee(
            gross_profit_usd,
            gas_units,
            base_fee_gwei,
            eth_price_usd,
        );

        Ok(AcrossBridgePrivateSubmissionPlan {
            spoke_pool_address: format!("{spoke_pool:#x}"),
            calldata,
            gas_tip_recommendation: gas_rec.clone(),
            recommended_relays: gas_rec.target_relay_endpoints,
        })
    }
}

/// Plan for private builder relay deposit submission.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AcrossBridgePrivateSubmissionPlan {
    pub spoke_pool_address: String,
    pub calldata: Vec<u8>,
    pub gas_tip_recommendation: arbitrageur_math::private_relay::PrivateRelayRecommendation,
    pub recommended_relays: Vec<String>,
}

/// Status of an Across V3 intent bridge deposit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AcrossDepositStatus {
    /// Deposit transaction broadcast or pending on origin chain.
    OriginPending,
    /// Deposit confirmed on origin chain (`V3FundsDeposited`).
    OriginConfirmed,
    /// Fast relayer has filled the intent on destination chain (`FilledRelay`).
    Filled,
    /// Fill deadline expired; refund issued on origin chain (`RefundedRelay`).
    Refunded,
    /// Fill deadline expired without fill or refund.
    Expired,
}

/// Decoded details from a `V3FundsDeposited` event on the origin chain.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecodedFundsDeposited {
    pub origin_chain_id: u64,
    pub destination_chain_id: u64,
    pub deposit_id: u32,
    pub depositor: String,
    pub recipient: String,
    pub input_token: String,
    pub output_token: String,
    pub input_amount_units: String,
    pub output_amount_units: String,
    pub quote_timestamp: u32,
    pub fill_deadline: u32,
    pub exclusivity_deadline: u32,
    pub exclusive_relayer: Option<String>,
}

/// Decoded details from a `FilledRelay` event on the destination chain.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecodedFilledRelay {
    pub origin_chain_id: u64,
    pub destination_chain_id: u64,
    pub deposit_id: u32,
    pub relayer: String,
    pub depositor: String,
    pub recipient: String,
    pub input_token: String,
    pub output_token: String,
    pub input_amount_units: String,
    pub output_amount_units: String,
}

/// Decoder for Across Protocol SpokePool on-chain events.
#[derive(Debug, Clone, Default)]
pub struct AcrossRelayEventDecoder;

impl AcrossRelayEventDecoder {
    /// Returns the topic0 hash for `V3FundsDeposited`.
    pub fn funds_deposited_topic0() -> B256 {
        IAcrossSpokePool::V3FundsDeposited::SIGNATURE_HASH
    }

    /// Returns the topic0 hash for `FilledRelay`.
    pub fn filled_relay_topic0() -> B256 {
        IAcrossSpokePool::FilledRelay::SIGNATURE_HASH
    }

    /// Returns the topic0 hash for `RefundedRelay`.
    pub fn refunded_relay_topic0() -> B256 {
        IAcrossSpokePool::RefundedRelay::SIGNATURE_HASH
    }

    /// Decodes a `V3FundsDeposited` event from raw log topics and unindexed data.
    pub fn decode_funds_deposited(
        origin_chain_id: u64,
        topics: &[B256],
        data: &[u8],
    ) -> Result<DecodedFundsDeposited, String> {
        let event =
            IAcrossSpokePool::V3FundsDeposited::decode_raw_log(topics.iter().copied(), data)
                .map_err(|e| format!("Failed to decode V3FundsDeposited event: {e}"))?;

        let dest_chain_id = event.destinationChainId.to::<u64>();
        let relayer_str = if event.exclusiveRelayer == Address::ZERO {
            None
        } else {
            Some(format!("{:#x}", event.exclusiveRelayer))
        };

        Ok(DecodedFundsDeposited {
            origin_chain_id,
            destination_chain_id: dest_chain_id,
            deposit_id: event.depositId,
            depositor: format!("{:#x}", event.depositor),
            recipient: format!("{:#x}", event.recipient),
            input_token: format!("{:#x}", event.inputToken),
            output_token: format!("{:#x}", event.outputToken),
            input_amount_units: event.inputAmount.to_string(),
            output_amount_units: event.outputAmount.to_string(),
            quote_timestamp: event.quoteTimestamp,
            fill_deadline: event.fillDeadline,
            exclusivity_deadline: event.exclusivityDeadline,
            exclusive_relayer: relayer_str,
        })
    }

    /// Decodes a `FilledRelay` event from raw log topics and unindexed data.
    pub fn decode_filled_relay(
        destination_chain_id: u64,
        topics: &[B256],
        data: &[u8],
    ) -> Result<DecodedFilledRelay, String> {
        let event = IAcrossSpokePool::FilledRelay::decode_raw_log(topics.iter().copied(), data)
            .map_err(|e| format!("Failed to decode FilledRelay event: {e}"))?;

        let origin_chain_id = event.originChainId.to::<u64>();

        Ok(DecodedFilledRelay {
            origin_chain_id,
            destination_chain_id,
            deposit_id: event.depositId,
            relayer: format!("{:#x}", event.relayer),
            depositor: format!("{:#x}", event.depositor),
            recipient: format!("{:#x}", event.recipient),
            input_token: format!("{:#x}", event.inputToken),
            output_token: format!("{:#x}", event.outputToken),
            input_amount_units: event.inputAmount.to_string(),
            output_amount_units: event.outputAmount.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_known_spoke_pool_addresses() {
        assert!(spoke_pool_address(1).is_some());
        assert!(spoke_pool_address(42161).is_some());
        assert!(spoke_pool_address(8453).is_some());
        assert!(spoke_pool_address(10).is_some());
        assert!(spoke_pool_address(99999).is_none());
    }

    #[test]
    fn test_encode_deposit_v3_calldata() {
        let adapter = AcrossIntentBridgeAdapter::new();
        let req = AcrossBridgeDepositRequest {
            origin_chain_id: 42161,
            destination_chain_id: 8453,
            depositor: "0x1111111111111111111111111111111111111111".into(),
            recipient: "0x2222222222222222222222222222222222222222".into(),
            input_token: "0xaf88d065e77c8cC2239327C5EDb3A432268e5831".into(), // USDC on Arb
            output_token: "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913".into(), // USDC on Base
            input_amount_units: "1000000000".into(), // 1,000 USDC (6 decimals)
            output_amount_units: "999500000".into(), // 999.5 USDC
            quote_timestamp: 1728000000,
            fill_deadline: 1728003600,
            exclusive_relayer: None,
            exclusivity_deadline: None,
            message: None,
        };

        let encoded = adapter.encode_deposit_v3(&req).expect("Encoding succeeds");
        assert!(
            encoded.len() > 32,
            "ABI encoded calldata must contain selector and arguments"
        );
        // Function selector for depositV3
        assert_eq!(encoded[0..4], IAcrossSpokePool::depositV3Call::SELECTOR);
    }

    #[test]
    fn test_encode_erc7683_open_calldata() {
        let adapter = AcrossIntentBridgeAdapter::new();
        let encoded = adapter.encode_erc7683_open(1728003600, [0u8; 32], vec![1, 2, 3, 4]);
        assert_eq!(encoded[0..4], IERC7683Settler::openCall::SELECTOR);
    }

    #[test]
    fn test_across_bridge_quote_evaluation() {
        let adapter = AcrossIntentBridgeAdapter::new();
        let quote = adapter.evaluate_quote(10000.0, 1.0);
        assert!(quote.fee_bps >= 3.0 && quote.fee_bps <= 7.0);
        assert!(quote.output_amount_usd < quote.input_amount_usd);
        assert!(quote.estimated_latency_sec >= 5.0 && quote.estimated_latency_sec <= 60.0);
    }

    #[test]
    fn test_prepare_private_deposit_submission() {
        let adapter = AcrossIntentBridgeAdapter::new();
        let req = AcrossBridgeDepositRequest {
            origin_chain_id: 42161,     // Arbitrum One
            destination_chain_id: 8453, // Base
            depositor: "0x1111111111111111111111111111111111111111".into(),
            recipient: "0x2222222222222222222222222222222222222222".into(),
            input_token: "0xaf88d065e77c8cC2239327C5EDb3A432268e5831".into(),
            output_token: "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913".into(),
            input_amount_units: "5000000000".into(), // 5,000 USDC
            output_amount_units: "4997500000".into(), // 4,997.5 USDC
            quote_timestamp: 1728000000,
            fill_deadline: 1728003600,
            exclusive_relayer: None,
            exclusivity_deadline: None,
            message: None,
        };

        let plan = adapter
            .prepare_private_deposit_submission(&req, 150.0, 240_000.0, 15.0, 3000.0)
            .expect("Plan generation succeeds");

        assert!(plan.spoke_pool_address.starts_with("0x"));
        assert!(!plan.calldata.is_empty());
        assert!(plan.gas_tip_recommendation.is_institutionally_compliant);
        assert!(plan.recommended_relays.len() >= 3);
    }

    #[test]
    fn test_encode_deposit_v3_unsupported_chain_rejected() {
        let adapter = AcrossIntentBridgeAdapter::new();
        let mut req = AcrossBridgeDepositRequest {
            origin_chain_id: 99999, // unsupported
            destination_chain_id: 8453,
            depositor: "0x1111111111111111111111111111111111111111".into(),
            recipient: "0x2222222222222222222222222222222222222222".into(),
            input_token: "0xaf88d065e77c8cC2239327C5EDb3A432268e5831".into(),
            output_token: "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913".into(),
            input_amount_units: "1000000000".into(),
            output_amount_units: "999500000".into(),
            quote_timestamp: 1728000000,
            fill_deadline: 1728003600,
            exclusive_relayer: None,
            exclusivity_deadline: None,
            message: None,
        };

        let err = adapter.encode_deposit_v3(&req).unwrap_err();
        assert!(err.contains("Unsupported origin chain ID 99999"));

        req.origin_chain_id = 42161;
        req.destination_chain_id = 88888; // unsupported dest
        let err2 = adapter.encode_deposit_v3(&req).unwrap_err();
        assert!(err2.contains("Unsupported destination chain ID 88888"));
    }

    #[test]
    fn test_across_event_decoding() {
        let deposit_event = IAcrossSpokePool::V3FundsDeposited {
            inputToken: "0xaf88d065e77c8cC2239327C5EDb3A432268e5831"
                .parse()
                .unwrap(),
            outputToken: "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"
                .parse()
                .unwrap(),
            inputAmount: U256::from(1_000_000_000u64),
            outputAmount: U256::from(999_500_000u64),
            destinationChainId: U256::from(8453u64),
            depositId: 42,
            quoteTimestamp: 1728000000,
            fillDeadline: 1728003600,
            exclusivityDeadline: 0,
            depositor: "0x1111111111111111111111111111111111111111"
                .parse()
                .unwrap(),
            recipient: "0x2222222222222222222222222222222222222222"
                .parse()
                .unwrap(),
            exclusiveRelayer: Address::ZERO,
            message: Bytes::default(),
        };

        let log_data = deposit_event.encode_log_data();
        let topics: Vec<B256> = log_data.topics().to_vec();
        let data = log_data.data.to_vec();

        let decoded = AcrossRelayEventDecoder::decode_funds_deposited(42161, &topics, &data)
            .expect("Decoding FundsDeposited succeeds");
        assert_eq!(decoded.origin_chain_id, 42161);
        assert_eq!(decoded.destination_chain_id, 8453);
        assert_eq!(decoded.deposit_id, 42);
        assert_eq!(decoded.input_amount_units, "1000000000");
        assert_eq!(decoded.output_amount_units, "999500000");

        let fill_event = IAcrossSpokePool::FilledRelay {
            inputToken: "0xaf88d065e77c8cC2239327C5EDb3A432268e5831"
                .parse()
                .unwrap(),
            outputToken: "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"
                .parse()
                .unwrap(),
            inputAmount: U256::from(1_000_000_000u64),
            outputAmount: U256::from(999_500_000u64),
            repaymentChainId: U256::from(1u64),
            originChainId: U256::from(42161u64),
            depositId: 42,
            fillDeadline: 1728003600,
            exclusivityDeadline: 0,
            exclusiveRelayer: Address::ZERO,
            relayer: "0x3333333333333333333333333333333333333333"
                .parse()
                .unwrap(),
            depositor: "0x1111111111111111111111111111111111111111"
                .parse()
                .unwrap(),
            recipient: "0x2222222222222222222222222222222222222222"
                .parse()
                .unwrap(),
            message: Bytes::default(),
        };

        let fill_log = fill_event.encode_log_data();
        let fill_topics: Vec<B256> = fill_log.topics().to_vec();
        let fill_data = fill_log.data.to_vec();

        let decoded_fill =
            AcrossRelayEventDecoder::decode_filled_relay(8453, &fill_topics, &fill_data)
                .expect("Decoding FilledRelay succeeds");
        assert_eq!(decoded_fill.origin_chain_id, 42161);
        assert_eq!(decoded_fill.destination_chain_id, 8453);
        assert_eq!(decoded_fill.deposit_id, 42);
        assert_eq!(decoded_fill.input_amount_units, "1000000000");
    }
}
