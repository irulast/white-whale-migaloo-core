use cosmwasm_std::{to_binary, Addr, Deps, QueryRequest, StdError, StdResult, WasmQuery};
use terraswap::asset::Asset;
use terraswap::factory::{PairsResponse, QueryMsg};
use terraswap::pair::ProtocolFeesResponse as ProtocolPairFeesResponse;
use vault_network::vault::ProtocolFeesResponse as ProtocolVaultFeesResponse;
use vault_network::vault_factory::VaultsResponse;

use crate::msg::{CollectFeesFor, FactoriesResponse, FactoryType};
use crate::state::{read_factories, ConfigResponse, CONFIG};

pub fn query_factories(deps: Deps, limit: Option<u32>) -> StdResult<FactoriesResponse> {
    let factories = read_factories(deps, limit)?;
    Ok(FactoriesResponse { factories })
}
/// Queries the [Config], which contains the owner address
pub fn query_config(deps: Deps) -> StdResult<ConfigResponse> {
    let config = CONFIG.load(deps.storage)?;
    Ok(config)
}

pub fn query_accrued_fees(deps: Deps, collect_fees_for: CollectFeesFor) -> StdResult<Vec<Asset>> {
    let mut query_fees_messages: Vec<Asset> = Vec::new();

    match collect_fees_for {
        CollectFeesFor::Contracts { .. } => {
            return Err(StdError::GenericErr {
                msg: "Query accrued fees for contracts is not allowed.".to_string(),
            })
        }
        CollectFeesFor::Factory {
            factory_addr,
            factory_type,
        } => {
            let factory = deps.api.addr_validate(factory_addr.as_str())?;
            let mut assets = query_fees_for_factory(&deps, &factory, factory_type)?;

            query_fees_messages.append(&mut assets);
        }
    }

    Ok(query_fees_messages)
}

fn query_fees_for_vault(deps: &Deps, vault: String) -> StdResult<ProtocolVaultFeesResponse> {
    deps.querier
        .query::<ProtocolVaultFeesResponse>(&QueryRequest::Wasm(WasmQuery::Smart {
            contract_addr: vault,
            msg: to_binary(&vault_network::vault::QueryMsg::ProtocolFees { all_time: false })?,
        }))
}

fn query_fees_for_pair(deps: &Deps, pair: String) -> StdResult<ProtocolPairFeesResponse> {
    deps.querier
        .query::<ProtocolPairFeesResponse>(&QueryRequest::Wasm(WasmQuery::Smart {
            contract_addr: pair,
            msg: to_binary(&terraswap::pair::QueryMsg::ProtocolFees {
                all_time: None,
                asset_id: None,
            })?,
        }))
}

fn query_fees_for_factory(
    deps: &Deps,
    factory: &Addr,
    factory_type: FactoryType,
) -> StdResult<Vec<Asset>> {
    let mut query_fees_messages: Vec<Asset> = Vec::new();

    match factory_type {
        FactoryType::Vault { start_after, limit } => {
            let response: VaultsResponse =
                deps.querier.query(&QueryRequest::Wasm(WasmQuery::Smart {
                    contract_addr: factory.to_string(),
                    msg: to_binary(&vault_network::vault_factory::QueryMsg::Vaults {
                        start_after,
                        limit,
                    })?,
                }))?;

            for vault_info in response.vaults {
                let vault_response = query_fees_for_vault(deps, vault_info.vault)?;
                query_fees_messages.push(vault_response.fees);
            }
        }
        FactoryType::Pool { start_after, limit } => {
            let response: PairsResponse =
                deps.querier.query(&QueryRequest::Wasm(WasmQuery::Smart {
                    contract_addr: factory.to_string(),
                    msg: to_binary(&QueryMsg::Pairs { start_after, limit })?,
                }))?;

            for pair in response.pairs {
                let mut pair_response = query_fees_for_pair(deps, pair.contract_addr)?;
                query_fees_messages.append(&mut pair_response.fees);
            }
        }
    }

    Ok(query_fees_messages)
}
