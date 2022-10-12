use cosmwasm_std::{Decimal, Uint128};
use cw20::Cw20ReceiveMsg;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use white_whale::fee::Fee;

use crate::asset::{Asset, AssetInfo};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
pub struct InstantiateMsg {
    /// Asset infos
    pub asset_infos: [AssetInfo; 2],
    /// Token contract code id for initialization
    pub token_code_id: u64,
    pub asset_decimals: [u8; 2],
    pub pool_fees: PoolFee,
    pub fee_collector_addr: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExecuteMsg {
    /// Used to trigger the [Cw20HookMsg] messages
    Receive(Cw20ReceiveMsg),
    /// Provides liquidity to the pool
    ProvideLiquidity {
        assets: [Asset; 2],
        slippage_tolerance: Option<Decimal>,
        receiver: Option<String>,
    },
    /// Swap an offer asset to the other
    Swap {
        offer_asset: Asset,
        belief_price: Option<Decimal>,
        max_spread: Option<Decimal>,
        to: Option<String>,
    },
    /// Updates the pair pool config
    UpdateConfig {
        owner: Option<String>,
        fee_collector_addr: Option<String>,
        pool_fees: Option<PoolFee>,
        feature_toggle: Option<FeatureToggle>,
    },
    /// Collects the Protocol fees accrued by the pool
    CollectProtocolFees {},
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Cw20HookMsg {
    /// Swaps a given amount of asset
    Swap {
        belief_price: Option<Decimal>,
        max_spread: Option<Decimal>,
        to: Option<String>,
    },
    /// Withdraws liquidity
    WithdrawLiquidity {},
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum QueryMsg {
    /// Retrieves the [PairInfo] for the pair.
    Pair {},
    /// Retrieves the configuration of the pool, returning a [ConfigResponse] response.
    Config {},
    /// Retrieves the protocol fees that have been accrued. If `all_time` is `true`, it will return
    /// the fees collected since the inception of the pool. On the other hand, if `all_time` is set
    /// to `false`, only the fees that has been accrued by the pool but not collected by the fee
    /// collector will be returned.
    ProtocolFees {
        asset_id: Option<String>,
        all_time: Option<bool>,
    },
    /// Retrieves the pool information, returning a [PoolResponse] response.
    Pool {},
    /// Simulates a swap, returns a [SimulationResponse] response.
    Simulation { offer_asset: Asset },
    /// Simulates a reverse swap, i.e. given the ask asset, how much of the offer asset is needed to
    /// perform the swap. Returns a [ReverseSimulationResponse] response.
    ReverseSimulation { ask_asset: Asset },
}

// Pool feature toggle
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
pub struct FeatureToggle {
    pub withdrawals_enabled: bool,
    pub deposits_enabled: bool,
    pub swaps_enabled: bool,
}

/// Fees used by the pools on the pool network
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq, JsonSchema)]
pub struct PoolFee {
    pub protocol_fee: Fee,
    pub swap_fee: Fee,
}

// We define a custom struct for each query response
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
pub struct PoolResponse {
    pub assets: [Asset; 2],
    pub total_share: Uint128,
}

/// SimulationResponse returns swap simulation response
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
pub struct SimulationResponse {
    pub return_amount: Uint128,
    pub spread_amount: Uint128,
    pub swap_fee_amount: Uint128,
    pub protocol_fee_amount: Uint128,
}

/// ReverseSimulationResponse returns reverse swap simulation response
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
pub struct ProtocolFeesResponse {
    pub fees: Vec<Asset>,
}

/// ReverseSimulationResponse returns reverse swap simulation response
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
pub struct ReverseSimulationResponse {
    pub offer_amount: Uint128,
    pub spread_amount: Uint128,
    pub swap_fee_amount: Uint128,
    pub protocol_fee_amount: Uint128,
}

/// We currently take no arguments for migrations
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
pub struct MigrateMsg {}
