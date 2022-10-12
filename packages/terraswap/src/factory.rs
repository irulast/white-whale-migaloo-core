use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::asset::{AssetInfo, PairInfo};
use crate::pair::PoolFee;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
pub struct InstantiateMsg {
    /// Pair contract code ID, which is used to
    pub pair_code_id: u64,
    pub token_code_id: u64,
    pub fee_collector_addr: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExecuteMsg {
    /// Updates contract's config, i.e. relevant code_ids, fee_collector address and owner
    UpdateConfig {
        owner: Option<String>,
        fee_collector_addr: Option<String>,
        token_code_id: Option<u64>,
        pair_code_id: Option<u64>,
    },
    /// Instantiates pair contract
    CreatePair {
        /// Asset infos
        asset_infos: [AssetInfo; 2],
        pool_fees: PoolFee,
    },
    /// Adds native token info to the contract so it can instantiate pair contracts that include it
    AddNativeTokenDecimals { denom: String, decimals: u8 },
    /// Migrates a pair contract to a given code_id
    MigratePair {
        contract: String,
        code_id: Option<u64>,
    },
    /// Removes pair
    RemovePair { pair_address: String },
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum QueryMsg {
    /// Retrieves the configuration of the contract in a [ConfigResponse] response.
    Config {},
    /// Retrieves the [PairInfo] for the given asset infos.
    Pair { asset_infos: [AssetInfo; 2] },
    /// Retrieves the Pairs created by the factory in a [PairsResponse] response. It returns ten
    /// results by default, though it has pagination parameters if needed. `start_after` contains the
    /// [AssetInfo] of the last item returned, while `limit` is the amount of items to retrieve, being
    /// 30 the max number.
    Pairs {
        start_after: Option<[AssetInfo; 2]>,
        limit: Option<u32>,
    },
    /// Retrieves the number of decimals for the given `denom`. The query fails if the denom is not found,
    /// i.e. if [AddNativeTokenDecimals] was not executed for the given denom.
    NativeTokenDecimals { denom: String },
}

// We define a custom struct for each query response
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
pub struct ConfigResponse {
    pub owner: String,
    pub fee_collector_addr: String,
    pub pair_code_id: u64,
    pub token_code_id: u64,
}

/// We currently take no arguments for migrations
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
pub struct MigrateMsg {}

// We define a custom struct for each query response
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
pub struct PairsResponse {
    pub pairs: Vec<PairInfo>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
pub struct NativeTokenDecimalsResponse {
    pub decimals: u8,
}
