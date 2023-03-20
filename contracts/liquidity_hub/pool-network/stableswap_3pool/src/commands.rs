use cosmwasm_std::{
    from_binary, to_binary, Addr, CosmosMsg, Decimal, DepsMut, Env, MessageInfo, OverflowError,
    Response, StdError, StdResult, Uint128, WasmMsg,
};
use cw20::{Cw20ExecuteMsg, Cw20ReceiveMsg};

use terraswap::asset::{Asset, AssetInfo, TrioInfoRaw, MINIMUM_LIQUIDITY_AMOUNT};
use terraswap::querier::query_token_info;
use terraswap::trio::{Config, Cw20HookMsg, FeatureToggle, PoolFee};

use crate::error::ContractError;
use crate::helpers;
use crate::helpers::get_protocol_fee_for_asset;
use crate::stableswap_math::curve::StableSwap;
use crate::state::{
    store_fee, ALL_TIME_BURNED_FEES, ALL_TIME_COLLECTED_PROTOCOL_FEES, COLLECTED_PROTOCOL_FEES,
    CONFIG, TRIO_INFO,
};

/// Receives cw20 tokens. Used to swap and withdraw from the pool.
pub fn receive_cw20(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    cw20_msg: Cw20ReceiveMsg,
) -> Result<Response, ContractError> {
    let contract_addr = info.sender.clone();
    let feature_toggle: FeatureToggle = CONFIG.load(deps.storage)?.feature_toggle;

    match from_binary(&cw20_msg.msg) {
        Ok(Cw20HookMsg::Swap {
            ask_asset,
            belief_price,
            max_spread,
            to,
        }) => {
            // check if the swap feature is enabled
            if !feature_toggle.swaps_enabled {
                return Err(ContractError::OperationDisabled("swap".to_string()));
            }

            // only asset contract can execute this message
            let mut authorized: bool = false;
            let config: TrioInfoRaw = TRIO_INFO.load(deps.storage)?;
            let pools: [Asset; 3] =
                config.query_pools(&deps.querier, deps.api, env.contract.address.clone())?;
            for pool in pools.iter() {
                if let AssetInfo::Token { contract_addr, .. } = &pool.info {
                    if contract_addr == &info.sender {
                        authorized = true;
                    }
                }
            }

            if !authorized {
                return Err(ContractError::Unauthorized {});
            }

            let to_addr = if let Some(to_addr) = to {
                Some(deps.api.addr_validate(to_addr.as_str())?)
            } else {
                None
            };

            swap(
                deps,
                env,
                info,
                Addr::unchecked(cw20_msg.sender),
                Asset {
                    info: AssetInfo::Token {
                        contract_addr: contract_addr.to_string(),
                    },
                    amount: cw20_msg.amount,
                },
                ask_asset,
                belief_price,
                max_spread,
                to_addr,
            )
        }
        Ok(Cw20HookMsg::WithdrawLiquidity {}) => {
            // check if the withdrawal feature is enabled
            if !feature_toggle.withdrawals_enabled {
                return Err(ContractError::OperationDisabled(
                    "withdraw_liquidity".to_string(),
                ));
            }

            let config: TrioInfoRaw = TRIO_INFO.load(deps.storage)?;
            if deps.api.addr_canonicalize(info.sender.as_str())? != config.liquidity_token {
                return Err(ContractError::Unauthorized {});
            }

            let sender_addr = deps.api.addr_validate(cw20_msg.sender.as_str())?;
            withdraw_liquidity(deps, env, info, sender_addr, cw20_msg.amount)
        }
        Err(err) => Err(ContractError::Std(err)),
    }
}

/// Provides liquidity. The user must IncreaseAllowance on the token when providing cw20 tokens
pub fn provide_liquidity(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    assets: [Asset; 3],
    slippage_tolerance: Option<Decimal>,
    receiver: Option<String>,
) -> Result<Response, ContractError> {
    let config = CONFIG.load(deps.storage)?;
    // check if the deposit feature is enabled
    if !config.feature_toggle.deposits_enabled {
        return Err(ContractError::OperationDisabled(
            "provide_liquidity".to_string(),
        ));
    }

    for asset in assets.iter() {
        asset.assert_sent_native_token_balance(&info)?;
    }

    let trio_info: TrioInfoRaw = TRIO_INFO.load(deps.storage)?;
    let mut pools: [Asset; 3] =
        trio_info.query_pools(&deps.querier, deps.api, env.contract.address.clone())?;
    let deposits: [Uint128; 3] = [
        assets
            .iter()
            .find(|a| a.info.equal(&pools[0].info))
            .map(|a| a.amount)
            .expect("Wrong asset info is given"),
        assets
            .iter()
            .find(|a| a.info.equal(&pools[1].info))
            .map(|a| a.amount)
            .expect("Wrong asset info is given"),
        assets
            .iter()
            .find(|a| a.info.equal(&pools[2].info))
            .map(|a| a.amount)
            .expect("Wrong asset info is given"),
    ];

    if deposits[0].is_zero() || deposits[1].is_zero() || deposits[2].is_zero() {
        return Err(ContractError::InvalidZeroAmount {});
    }

    let mut messages: Vec<CosmosMsg> = vec![];
    for (i, pool) in pools.iter_mut().enumerate() {
        // If the pool is token contract, then we need to execute TransferFrom msg to receive funds
        if let AssetInfo::Token { contract_addr, .. } = &pool.info {
            messages.push(CosmosMsg::Wasm(WasmMsg::Execute {
                contract_addr: contract_addr.to_string(),
                msg: to_binary(&Cw20ExecuteMsg::TransferFrom {
                    owner: info.sender.to_string(),
                    recipient: env.contract.address.to_string(),
                    amount: deposits[i],
                })?,
                funds: vec![],
            }));
        } else {
            // If the asset is native token, balance is already increased
            // To calculate it properly we should subtract user deposit from the pool
            pool.amount = pool.amount.checked_sub(deposits[i])?;
        }
    }

    // deduct protocol fee from pools
    let collected_protocol_fees = COLLECTED_PROTOCOL_FEES.load(deps.storage)?;
    for pool in pools.iter_mut() {
        let protocol_fee =
            get_protocol_fee_for_asset(collected_protocol_fees.clone(), pool.clone().get_id());
        pool.amount = pool.amount.checked_sub(protocol_fee)?;
    }

    // assert slippage tolerance
    helpers::assert_slippage_tolerance(&slippage_tolerance, &deposits, &pools)?;

    let liquidity_token = deps.api.addr_humanize(&trio_info.liquidity_token)?;
    let total_share = query_token_info(&deps.querier, liquidity_token)?.total_supply;
    let invariant = StableSwap::new(config.amp_factor, config.amp_factor, 0, 0, 0);
    let share = if total_share == Uint128::zero() {
        // Make sure at least MINIMUM_LIQUIDITY_AMOUNT is deposited to mitigate the risk of the first
        // depositor preventing small liquidity providers from joining the pool
        let min_lp_token_amount = MINIMUM_LIQUIDITY_AMOUNT * Uint128::from(3u8);
        let share = Uint128::try_from(
            invariant
                .compute_d(deposits[0], deposits[1], deposits[2])
                .unwrap(),
        )
        .unwrap()
        .checked_sub(min_lp_token_amount)
        .map_err(|_| ContractError::InvalidInitialLiquidityAmount(min_lp_token_amount))?;

        messages.push(mint_lp_token_msg(
            deps.api
                .addr_humanize(&trio_info.liquidity_token)?
                .to_string(),
            env.contract.address.to_string(),
            min_lp_token_amount,
        )?);
        share
    } else {
        invariant
            .compute_mint_amount_for_deposit(
                deposits[0],
                deposits[1],
                deposits[2],
                pools[0].amount,
                pools[1].amount,
                pools[2].amount,
                total_share,
            )
            .unwrap()
    };

    // mint LP token to sender
    let receiver = receiver.unwrap_or_else(|| info.sender.to_string());
    messages.push(mint_lp_token_msg(
        deps.api
            .addr_humanize(&trio_info.liquidity_token)?
            .to_string(),
        receiver.clone(),
        share,
    )?);

    Ok(Response::new().add_messages(messages).add_attributes(vec![
        ("action", "provide_liquidity"),
        ("sender", info.sender.as_str()),
        ("receiver", receiver.as_str()),
        ("assets", &format!("{}, {}", assets[0], assets[1])),
        ("share", &share.to_string()),
    ]))
}

/// Withdraws the liquidity. The user burns the LP tokens in exchange for the tokens provided, including
/// the swap fees accrued by its share of the pool.
pub fn withdraw_liquidity(
    deps: DepsMut,
    env: Env,
    _info: MessageInfo,
    sender: Addr,
    amount: Uint128,
) -> Result<Response, ContractError> {
    let trio_info: TrioInfoRaw = TRIO_INFO.load(deps.storage)?;
    let liquidity_addr: Addr = deps.api.addr_humanize(&trio_info.liquidity_token)?;

    let pool_assets: [Asset; 3] =
        trio_info.query_pools(&deps.querier, deps.api, env.contract.address)?;
    let total_share: Uint128 = query_token_info(&deps.querier, liquidity_addr)?.total_supply;

    let collected_protocol_fees = COLLECTED_PROTOCOL_FEES.load(deps.storage)?;

    let share_ratio: Decimal = Decimal::from_ratio(amount, total_share);

    let refund_assets: Result<Vec<Asset>, OverflowError> = pool_assets
        .iter()
        .map(|pool_asset| {
            let protocol_fee = get_protocol_fee_for_asset(
                collected_protocol_fees.clone(),
                pool_asset.clone().get_id(),
            );

            // subtract the protocol_fee from the amount of the pool_asset
            let refund_amount = pool_asset.amount.checked_sub(protocol_fee)?;
            Ok(Asset {
                info: pool_asset.info.clone(),
                amount: refund_amount * share_ratio,
            })
        })
        .collect();

    let refund_assets = refund_assets?;

    // update pool info
    Ok(Response::new()
        .add_messages(vec![
            refund_assets[0].clone().into_msg(sender.clone())?,
            refund_assets[1].clone().into_msg(sender.clone())?,
            refund_assets[2].clone().into_msg(sender.clone())?,
            // burn liquidity token
            CosmosMsg::Wasm(WasmMsg::Execute {
                contract_addr: deps
                    .api
                    .addr_humanize(&trio_info.liquidity_token)?
                    .to_string(),
                msg: to_binary(&Cw20ExecuteMsg::Burn { amount })?,
                funds: vec![],
            }),
        ])
        .add_attributes(vec![
            ("action", "withdraw_liquidity"),
            ("sender", sender.as_str()),
            ("withdrawn_share", &amount.to_string()),
            (
                "refund_assets",
                &format!(
                    "{}, {}, {}",
                    refund_assets[0], refund_assets[1], refund_assets[2]
                ),
            ),
        ]))
}

/// Swaps tokens. The user must IncreaseAllowance on the token if it is a cw20 token they want to swa
#[allow(clippy::too_many_arguments)]
pub fn swap(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    sender: Addr,
    offer_asset: Asset,
    ask_asset: Asset,
    belief_price: Option<Decimal>,
    max_spread: Option<Decimal>,
    to: Option<Addr>,
) -> Result<Response, ContractError> {
    offer_asset.assert_sent_native_token_balance(&info)?;

    let trio_info: TrioInfoRaw = TRIO_INFO.load(deps.storage)?;

    // determine what's the offer and ask pool based on the offer_asset
    let collected_protocol_fees = COLLECTED_PROTOCOL_FEES.load(deps.storage)?;

    // To calculate pool amounts properly we should subtract user deposit and the protocol fees from the pool
    let pools = trio_info
        .query_pools(&deps.querier, deps.api, env.contract.address)?
        .into_iter()
        .map(|mut pool| {
            // subtract the protocol fee from the pool
            let protocol_fee =
                get_protocol_fee_for_asset(collected_protocol_fees.clone(), pool.clone().get_id());
            pool.amount = pool.amount.checked_sub(protocol_fee)?;

            if pool.info.equal(&offer_asset.info) {
                pool.amount = pool.amount.checked_sub(offer_asset.amount)?
            }

            Ok(pool)
        })
        .collect::<StdResult<Vec<_>>>()?;

    let ask_pool: Asset;
    let offer_pool: Asset;
    let unswapped_pool: Asset;
    let ask_decimal: u8;
    let offer_decimal: u8;

    if ask_asset.info.equal(&pools[0].info) {
        if offer_asset.info.equal(&pools[1].info) {
            ask_pool = pools[0].clone();
            offer_pool = pools[1].clone();
            unswapped_pool = pools[2].clone();

            ask_decimal = trio_info.asset_decimals[0];
            offer_decimal = trio_info.asset_decimals[1];
        } else if offer_asset.info.equal(&pools[2].info) {
            ask_pool = pools[0].clone();
            offer_pool = pools[2].clone();
            unswapped_pool = pools[1].clone();

            ask_decimal = trio_info.asset_decimals[0];
            offer_decimal = trio_info.asset_decimals[2];
        } else {
            return Err(ContractError::AssetMismatch {});
        }
    } else if ask_asset.info.equal(&pools[1].info) {
        if offer_asset.info.equal(&pools[0].info) {
            ask_pool = pools[1].clone();
            offer_pool = pools[0].clone();
            unswapped_pool = pools[2].clone();

            ask_decimal = trio_info.asset_decimals[1];
            offer_decimal = trio_info.asset_decimals[0];
        } else if offer_asset.info.equal(&pools[2].info) {
            ask_pool = pools[1].clone();
            offer_pool = pools[2].clone();
            unswapped_pool = pools[0].clone();

            ask_decimal = trio_info.asset_decimals[1];
            offer_decimal = trio_info.asset_decimals[2];
        } else {
            return Err(ContractError::AssetMismatch {});
        }
    } else if ask_asset.info.equal(&pools[2].info) {
        if offer_asset.info.equal(&pools[0].info) {
            ask_pool = pools[2].clone();
            offer_pool = pools[0].clone();
            unswapped_pool = pools[1].clone();

            ask_decimal = trio_info.asset_decimals[2];
            offer_decimal = trio_info.asset_decimals[0];
        } else if offer_asset.info.equal(&pools[1].info) {
            ask_pool = pools[2].clone();
            offer_pool = pools[1].clone();
            unswapped_pool = pools[0].clone();

            ask_decimal = trio_info.asset_decimals[2];
            offer_decimal = trio_info.asset_decimals[1];
        } else {
            return Err(ContractError::AssetMismatch {});
        }
    } else {
        return Err(ContractError::AssetMismatch {});
    }

    let offer_amount = offer_asset.amount;
    let config = CONFIG.load(deps.storage)?;

    let swap_computation = helpers::compute_swap(
        offer_pool.amount,
        ask_pool.amount,
        unswapped_pool.amount,
        offer_amount,
        config.pool_fees,
        config.amp_factor,
    )?;

    let return_asset = Asset {
        info: ask_pool.info.clone(),
        amount: swap_computation.return_amount,
    };

    // check max spread limit if exist
    helpers::assert_max_spread(
        belief_price,
        max_spread,
        offer_asset.clone(),
        return_asset.clone(),
        swap_computation.spread_amount,
        offer_decimal,
        ask_decimal,
    )?;

    let receiver = to.unwrap_or_else(|| sender.clone());

    let mut messages: Vec<CosmosMsg> = vec![];
    if !swap_computation.return_amount.is_zero() {
        messages.push(return_asset.into_msg(receiver.clone())?);
    }

    // burn ask_asset from the pool
    if !swap_computation.burn_fee_amount.is_zero() {
        let burn_asset = Asset {
            info: ask_pool.info.clone(),
            amount: swap_computation.burn_fee_amount,
        };

        store_fee(
            deps.storage,
            burn_asset.amount,
            burn_asset.clone().get_id(),
            ALL_TIME_BURNED_FEES,
        )?;

        messages.push(burn_asset.into_burn_msg()?);
    }

    // Store the protocol fees generated by this swap. The protocol fees are collected on the ask
    // asset as shown in [compute_swap]
    store_fee(
        deps.storage,
        swap_computation.protocol_fee_amount,
        ask_pool.clone().get_id(),
        COLLECTED_PROTOCOL_FEES,
    )?;
    store_fee(
        deps.storage,
        swap_computation.protocol_fee_amount,
        ask_pool.clone().get_id(),
        ALL_TIME_COLLECTED_PROTOCOL_FEES,
    )?;

    // 1. send collateral token from the contract to a user
    // 2. stores the protocol fees
    Ok(Response::new().add_messages(messages).add_attributes(vec![
        ("action", "swap"),
        ("sender", sender.as_str()),
        ("receiver", receiver.as_str()),
        ("offer_asset", &offer_asset.info.to_string()),
        ("ask_asset", &ask_pool.info.to_string()),
        ("offer_amount", &offer_amount.to_string()),
        ("return_amount", &swap_computation.return_amount.to_string()),
        ("spread_amount", &swap_computation.spread_amount.to_string()),
        (
            "swap_fee_amount",
            &swap_computation.swap_fee_amount.to_string(),
        ),
        (
            "protocol_fee_amount",
            &swap_computation.protocol_fee_amount.to_string(),
        ),
        (
            "burn_fee_amount",
            &swap_computation.burn_fee_amount.to_string(),
        ),
    ]))
}

/// Updates the [Config] of the contract. Only the owner of the contract can do this.
pub fn update_config(
    deps: DepsMut,
    info: MessageInfo,
    owner: Option<String>,
    fee_collector_addr: Option<String>,
    pool_fees: Option<PoolFee>,
    feature_toggle: Option<FeatureToggle>,
    amp_factor: Option<u64>,
) -> Result<Response, ContractError> {
    let mut config: Config = CONFIG.load(deps.storage)?;
    if deps.api.addr_validate(info.sender.as_str())? != config.owner {
        return Err(ContractError::Std(StdError::generic_err("unauthorized")));
    }

    if let Some(owner) = owner {
        // validate address format
        let _ = deps.api.addr_validate(&owner)?;
        config.owner = deps.api.addr_validate(&owner)?;
    }

    if let Some(pool_fees) = pool_fees {
        pool_fees.is_valid()?;
        config.pool_fees = pool_fees;
    }

    if let Some(feature_toggle) = feature_toggle {
        config.feature_toggle = feature_toggle;
    }

    if let Some(amp_factor) = amp_factor {
        config.amp_factor = amp_factor;
    }

    if let Some(fee_collector_addr) = fee_collector_addr {
        config.fee_collector_addr = deps.api.addr_validate(fee_collector_addr.as_str())?;
    }

    CONFIG.save(deps.storage, &config)?;

    Ok(Response::new().add_attribute("action", "update_config"))
}

/// Collects all protocol fees accrued by the pool
pub fn collect_protocol_fees(deps: DepsMut) -> Result<Response, ContractError> {
    let config = CONFIG.load(deps.storage)?;

    // get the collected protocol fees so far
    let protocol_fees = COLLECTED_PROTOCOL_FEES.load(deps.storage)?;
    // reset the collected protocol fees
    COLLECTED_PROTOCOL_FEES.save(
        deps.storage,
        &vec![
            Asset {
                info: protocol_fees[0].clone().info,
                amount: Uint128::zero(),
            },
            Asset {
                info: protocol_fees[1].clone().info,
                amount: Uint128::zero(),
            },
            Asset {
                info: protocol_fees[2].clone().info,
                amount: Uint128::zero(),
            },
        ],
    )?;

    let mut messages: Vec<CosmosMsg> = Vec::new();
    for protocol_fee in protocol_fees {
        // prevents trying to send 0 coins, which errors
        if protocol_fee.amount != Uint128::zero() {
            messages.push(protocol_fee.into_msg(config.fee_collector_addr.clone())?);
        }
    }

    Ok(Response::new()
        .add_attribute("action", "collect_protocol_fees")
        .add_messages(messages))
}

/// Creates the Mint LP message
fn mint_lp_token_msg(
    lp_token_addr: String,
    recipient: String,
    amount: Uint128,
) -> Result<CosmosMsg, ContractError> {
    Ok(CosmosMsg::Wasm(WasmMsg::Execute {
        contract_addr: lp_token_addr,
        msg: to_binary(&Cw20ExecuteMsg::Mint { recipient, amount })?,
        funds: vec![],
    }))
}
