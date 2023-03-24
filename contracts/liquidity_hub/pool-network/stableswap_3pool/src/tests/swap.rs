use crate::contract::{execute, instantiate, query, reply};
use crate::error::ContractError;
use crate::helpers::compute_swap;
use crate::queries::query_fees;
use crate::stableswap_math::curve::StableSwap;
use crate::state::{
    ALL_TIME_BURNED_FEES, ALL_TIME_COLLECTED_PROTOCOL_FEES, COLLECTED_PROTOCOL_FEES,
};
use cosmwasm_std::testing::{mock_env, mock_info, MOCK_CONTRACT_ADDR};
use cosmwasm_std::{
    attr, coins, from_binary, to_binary, BankMsg, Coin, CosmosMsg, Decimal, Reply, ReplyOn, SubMsg,
    SubMsgResponse, SubMsgResult, Uint128, WasmMsg,
};
use cw20::{Cw20ExecuteMsg, Cw20ReceiveMsg};
use terraswap::asset::{Asset, AssetInfo};
use terraswap::mock_querier::mock_dependencies;
use terraswap::trio::{
    Cw20HookMsg, ExecuteMsg, InstantiateMsg, PoolFee, QueryMsg, ReverseSimulationResponse,
    SimulationResponse,
};
use white_whale::fee::Fee;

#[test]
fn test_compute_swap_with_huge_pool_variance() {
    let offer_pool = Uint128::from(395451850234u128);
    let ask_pool = Uint128::from(317u128);
    let unswapped_pool = Uint128::from(1234567u128);
    let pool_fees = PoolFee {
        protocol_fee: Fee {
            share: Decimal::percent(1u64),
        },
        swap_fee: Fee {
            share: Decimal::percent(1u64),
        },
        burn_fee: Fee {
            share: Decimal::zero(),
        },
    };

    assert_eq!(
        compute_swap(
            offer_pool,
            ask_pool,
            unswapped_pool,
            Uint128::from(1u128),
            pool_fees,
            StableSwap::new(1000, 1000, 0, 0, 0)
        )
        .unwrap()
        .return_amount,
        Uint128::zero()
    );
}

#[test]
fn try_native_to_token() {
    let total_share = Uint128::from(30000000000u128);
    let asset_pool_amount = Uint128::from(20000000000u128);
    let collateral_pool_amount = Uint128::from(30000000000u128);
    let offer_amount = Uint128::from(1500000000u128);

    let mut deps = mock_dependencies(&[Coin {
        denom: "uusd".to_string(),
        amount: collateral_pool_amount + offer_amount,
        /* user deposit must be pre-applied */
    }]);

    deps.querier.with_token_balances(&[
        (
            &"liquidity0000".to_string(),
            &[(&MOCK_CONTRACT_ADDR.to_string(), &total_share)],
        ),
        (
            &"asset0000".to_string(),
            &[(&MOCK_CONTRACT_ADDR.to_string(), &asset_pool_amount)],
        ),
        (
            &"asset0001".to_string(),
            &[(&MOCK_CONTRACT_ADDR.to_string(), &asset_pool_amount)],
        ),
    ]);

    let msg = InstantiateMsg {
        asset_infos: [
            AssetInfo::NativeToken {
                denom: "uusd".to_string(),
            },
            AssetInfo::Token {
                contract_addr: "asset0000".to_string(),
            },
            AssetInfo::Token {
                contract_addr: "asset0001".to_string(),
            },
        ],
        token_code_id: 10u64,
        asset_decimals: [6u8, 8u8, 10u8],
        pool_fees: PoolFee {
            protocol_fee: Fee {
                share: Decimal::from_ratio(1u128, 1000u128),
            },
            swap_fee: Fee {
                share: Decimal::from_ratio(3u128, 1000u128),
            },
            burn_fee: Fee {
                share: Decimal::from_ratio(1u128, 1000u128),
            },
        },
        fee_collector_addr: "collector".to_string(),
        amp_factor: 1000,
    };

    let env = mock_env();
    let info = mock_info("addr0000", &[]);
    // we can just call .unwrap() to assert this was a success
    let _res = instantiate(deps.as_mut(), env, info, msg).unwrap();

    // store liquidity token
    let reply_msg = Reply {
        id: 1,
        result: SubMsgResult::Ok(SubMsgResponse {
            events: vec![],
            data: Some(
                vec![
                    10, 13, 108, 105, 113, 117, 105, 100, 105, 116, 121, 48, 48, 48, 48,
                ]
                .into(),
            ),
        }),
    };

    let _res = reply(deps.as_mut(), mock_env(), reply_msg).unwrap();

    // normal swap
    let msg = ExecuteMsg::Swap {
        offer_asset: Asset {
            info: AssetInfo::NativeToken {
                denom: "uusd".to_string(),
            },
            amount: offer_amount,
        },
        ask_asset: AssetInfo::Token {
            contract_addr: "asset0000".to_string(),
        },
        belief_price: None,
        max_spread: None,
        to: None,
    };
    let env = mock_env();
    let info = mock_info(
        "addr0000",
        &[Coin {
            denom: "uusd".to_string(),
            amount: offer_amount,
        }],
    );
    let res = execute(deps.as_mut(), env, info, msg).unwrap();
    assert_eq!(res.messages.len(), 2);
    let msg_transfer = res.messages.get(0).expect("no message");

    //Expected return on a stable swap should about the same as input,
    // swap fee will be approximately 1_500_000_000 * 0.003   4_500_000
    // burn and protocol fees will be approximately 1_500_000_000 * 0.001 1_500_000
    let expected_spread_amount = Uint128::from(730_134u128);
    let expected_swap_fee_amount = Uint128::from(4_497_809u128);
    let expected_protocol_fee_amount = Uint128::from(1_499_269u128);
    let expected_burn_fee_amount = Uint128::from(1_499_269u128);
    let expected_return_amount = Uint128::from(1_491_773_519u128);

    // since there is a burn_fee on the PoolFee, check burn message
    // since we swapped to a cw20 token, the burn message should be a Cw20ExecuteMsg::Burn
    let expected_burn_msg = SubMsg {
        id: 0,
        msg: CosmosMsg::Wasm(WasmMsg::Execute {
            contract_addr: "asset0000".to_string(),
            msg: to_binary(&Cw20ExecuteMsg::Burn {
                amount: expected_burn_fee_amount,
            })
            .unwrap(),
            funds: vec![],
        }),
        gas_limit: None,
        reply_on: ReplyOn::Never,
    };
    assert_eq!(res.messages.last().unwrap().clone(), expected_burn_msg);

    // as we swapped native to token, we accumulate the protocol fees in token
    let protocol_fees_for_token = query_fees(
        deps.as_ref(),
        Some("asset0000".to_string()),
        None,
        COLLECTED_PROTOCOL_FEES,
        Some(ALL_TIME_COLLECTED_PROTOCOL_FEES),
    )
    .unwrap()
    .fees;
    assert_eq!(
        protocol_fees_for_token.first().unwrap().amount,
        expected_protocol_fee_amount
    );
    let burned_fees_for_token = query_fees(
        deps.as_ref(),
        Some("asset0000".to_string()),
        None,
        ALL_TIME_BURNED_FEES,
        None,
    )
    .unwrap()
    .fees;
    assert_eq!(
        burned_fees_for_token.first().unwrap().amount,
        expected_burn_fee_amount
    );
    let protocol_fees_for_native = query_fees(
        deps.as_ref(),
        Some("uusd".to_string()),
        None,
        COLLECTED_PROTOCOL_FEES,
        Some(ALL_TIME_COLLECTED_PROTOCOL_FEES),
    )
    .unwrap()
    .fees;
    assert_eq!(
        protocol_fees_for_native.first().unwrap().amount,
        Uint128::zero()
    );
    let burned_fees_for_native = query_fees(
        deps.as_ref(),
        Some("uusd".to_string()),
        None,
        ALL_TIME_BURNED_FEES,
        None,
    )
    .unwrap()
    .fees;
    assert_eq!(
        burned_fees_for_native.first().unwrap().amount,
        Uint128::zero()
    );

    // check simulation res, reset values pre-swap to check simulation
    deps.querier.with_balance(&[(
        &MOCK_CONTRACT_ADDR.to_string(),
        vec![Coin {
            denom: "uusd".to_string(),
            amount: collateral_pool_amount,
            /* user deposit must be pre-applied */
        }],
    )]);

    // reset protocol fees so the simulation returns same values as the actual swap
    COLLECTED_PROTOCOL_FEES
        .save(
            &mut deps.storage,
            &vec![
                Asset {
                    info: AssetInfo::NativeToken {
                        denom: "uusd".to_string(),
                    },
                    amount: Uint128::zero(),
                },
                Asset {
                    info: AssetInfo::Token {
                        contract_addr: "asset0000".to_string(),
                    },
                    amount: Uint128::zero(),
                },
            ],
        )
        .unwrap();

    let simulation_res: SimulationResponse = from_binary(
        &query(
            deps.as_ref(),
            mock_env(),
            QueryMsg::Simulation {
                offer_asset: Asset {
                    info: AssetInfo::NativeToken {
                        denom: "uusd".to_string(),
                    },
                    amount: offer_amount,
                },
                ask_asset: Asset {
                    info: AssetInfo::Token {
                        contract_addr: "asset0000".to_string(),
                    },
                    amount: Default::default(),
                },
            },
        )
        .unwrap(),
    )
    .unwrap();

    assert_eq!(expected_return_amount, simulation_res.return_amount);
    assert_eq!(expected_swap_fee_amount, simulation_res.swap_fee_amount);
    assert_eq!(expected_burn_fee_amount, simulation_res.burn_fee_amount);
    assert_eq!(expected_spread_amount, simulation_res.spread_amount);
    assert_eq!(
        expected_protocol_fee_amount,
        simulation_res.protocol_fee_amount
    );

    // reset protocol fees so the simulation returns same values as the actual swap
    COLLECTED_PROTOCOL_FEES
        .save(
            &mut deps.storage,
            &vec![
                Asset {
                    info: AssetInfo::NativeToken {
                        denom: "uusd".to_string(),
                    },
                    amount: Uint128::zero(),
                },
                Asset {
                    info: AssetInfo::Token {
                        contract_addr: "asset0000".to_string(),
                    },
                    amount: Uint128::zero(),
                },
            ],
        )
        .unwrap();

    let reverse_simulation_res: ReverseSimulationResponse = from_binary(
        &query(
            deps.as_ref(),
            mock_env(),
            QueryMsg::ReverseSimulation {
                ask_asset: Asset {
                    info: AssetInfo::Token {
                        contract_addr: "asset0000".to_string(),
                    },
                    amount: expected_return_amount,
                },
                offer_asset: Asset {
                    info: AssetInfo::NativeToken {
                        denom: "uusd".to_string(),
                    },
                    amount: Default::default(),
                },
            },
        )
        .unwrap(),
    )
    .unwrap();

    assert!(
        (offer_amount.u128() as i128 - reverse_simulation_res.offer_amount.u128() as i128).abs()
            < 3i128
    );
    assert!(
        (expected_swap_fee_amount.u128() as i128
            - reverse_simulation_res.swap_fee_amount.u128() as i128)
            .abs()
            < 3i128
    );
    assert!(
        (expected_spread_amount.u128() as i128
            - reverse_simulation_res.spread_amount.u128() as i128)
            .abs()
            < 3i128
    );
    assert!(
        (expected_protocol_fee_amount.u128() as i128
            - reverse_simulation_res.protocol_fee_amount.u128() as i128)
            .abs()
            < 3i128
    );
    assert!(
        (expected_burn_fee_amount.u128() as i128
            - reverse_simulation_res.burn_fee_amount.u128() as i128)
            .abs()
            < 3i128
    );

    assert_eq!(
        res.attributes,
        vec![
            attr("action", "swap"),
            attr("sender", "addr0000"),
            attr("receiver", "addr0000"),
            attr("offer_asset", "uusd"),
            attr("ask_asset", "asset0000"),
            attr("offer_amount", offer_amount.to_string()),
            attr("return_amount", expected_return_amount.to_string()),
            attr("spread_amount", expected_spread_amount.to_string()),
            attr("swap_fee_amount", expected_swap_fee_amount.to_string()),
            attr(
                "protocol_fee_amount",
                expected_protocol_fee_amount.to_string(),
            ),
            attr("burn_fee_amount", expected_burn_fee_amount.to_string(),),
        ]
    );

    assert_eq!(
        &SubMsg::new(CosmosMsg::Wasm(WasmMsg::Execute {
            contract_addr: "asset0000".to_string(),
            msg: to_binary(&Cw20ExecuteMsg::Transfer {
                recipient: "addr0000".to_string(),
                amount: expected_return_amount,
            })
            .unwrap(),
            funds: vec![],
        })),
        msg_transfer,
    );
}

#[test]
fn try_swap_invalid_token() {
    let total_share = Uint128::from(30000000000u128);
    let asset_pool_amount = Uint128::from(20000000000u128);
    let collateral_pool_amount = Uint128::from(30000000000u128);
    let offer_amount = Uint128::from(1500000000u128);

    let mut deps = mock_dependencies(&[Coin {
        denom: "uusd".to_string(),
        amount: collateral_pool_amount + offer_amount,
        /* user deposit must be pre-applied */
    }]);

    deps.querier.with_token_balances(&[
        (
            &"liquidity0000".to_string(),
            &[(&MOCK_CONTRACT_ADDR.to_string(), &total_share)],
        ),
        (
            &"asset0000".to_string(),
            &[(&MOCK_CONTRACT_ADDR.to_string(), &asset_pool_amount)],
        ),
        (
            &"asset0001".to_string(),
            &[(&MOCK_CONTRACT_ADDR.to_string(), &asset_pool_amount)],
        ),
    ]);

    let msg = InstantiateMsg {
        asset_infos: [
            AssetInfo::NativeToken {
                denom: "uusd".to_string(),
            },
            AssetInfo::Token {
                contract_addr: "asset0000".to_string(),
            },
            AssetInfo::Token {
                contract_addr: "asset0001".to_string(),
            },
        ],
        token_code_id: 10u64,
        asset_decimals: [6u8, 8u8, 10u8],
        pool_fees: PoolFee {
            protocol_fee: Fee {
                share: Decimal::from_ratio(1u128, 1000u128),
            },
            swap_fee: Fee {
                share: Decimal::from_ratio(3u128, 1000u128),
            },
            burn_fee: Fee {
                share: Decimal::zero(),
            },
        },
        fee_collector_addr: "collector".to_string(),
        amp_factor: 1000,
    };

    let env = mock_env();
    let info = mock_info("addr0000", &[]);
    // we can just call .unwrap() to assert this was a success
    let _res = instantiate(deps.as_mut(), env, info, msg).unwrap();

    // store liquidity token
    let reply_msg = Reply {
        id: 1,
        result: SubMsgResult::Ok(SubMsgResponse {
            events: vec![],
            data: Some(
                vec![
                    10, 13, 108, 105, 113, 117, 105, 100, 105, 116, 121, 48, 48, 48, 48,
                ]
                .into(),
            ),
        }),
    };

    let _res = reply(deps.as_mut(), mock_env(), reply_msg).unwrap();

    // normal swap
    let msg = ExecuteMsg::Swap {
        offer_asset: Asset {
            info: AssetInfo::NativeToken {
                denom: "invalid_token".to_string(),
            },
            amount: offer_amount,
        },
        ask_asset: AssetInfo::Token {
            contract_addr: "asset0000".to_string(),
        },
        belief_price: None,
        max_spread: None,
        to: None,
    };
    let env = mock_env();
    let info = mock_info(
        "addr0000",
        &[Coin {
            denom: "invalid_token".to_string(),
            amount: offer_amount,
        }],
    );
    let res = execute(deps.as_mut(), env, info, msg);

    match res {
        Ok(_) => panic!("should return ContractError::AssetMismatch"),
        Err(ContractError::AssetMismatch {}) => (),
        _ => panic!("should return ContractError::AssetMismatch"),
    }
}

#[test]
fn try_token_to_native() {
    let total_share = Uint128::from(20_000_000_000u128);
    let asset_pool_amount = Uint128::from(30_000_000_000u128);
    let collateral_pool_amount = Uint128::from(20_000_000_000u128);
    let offer_amount = Uint128::from(1_500_000_000u128);

    let mut deps = mock_dependencies(&[Coin {
        denom: "uusd".to_string(),
        amount: collateral_pool_amount,
    }]);
    deps.querier.with_token_balances(&[
        (
            &"liquidity0000".to_string(),
            &[(&MOCK_CONTRACT_ADDR.to_string(), &total_share)],
        ),
        (
            &"asset0000".to_string(),
            &[(
                &MOCK_CONTRACT_ADDR.to_string(),
                &(asset_pool_amount + offer_amount),
            )],
        ),
        (
            &"asset0001".to_string(),
            &[(&MOCK_CONTRACT_ADDR.to_string(), &(asset_pool_amount))],
        ),
    ]);

    let msg = InstantiateMsg {
        asset_infos: [
            AssetInfo::NativeToken {
                denom: "uusd".to_string(),
            },
            AssetInfo::Token {
                contract_addr: "asset0000".to_string(),
            },
            AssetInfo::Token {
                contract_addr: "asset0001".to_string(),
            },
        ],
        token_code_id: 10u64,
        asset_decimals: [8u8, 8u8, 10u8],
        pool_fees: PoolFee {
            protocol_fee: Fee {
                share: Decimal::from_ratio(1u128, 1000u128),
            },
            swap_fee: Fee {
                share: Decimal::from_ratio(3u128, 1000u128),
            },
            burn_fee: Fee {
                share: Decimal::from_ratio(1u128, 1000u128),
            },
        },
        fee_collector_addr: "collector".to_string(),
        amp_factor: 1000,
    };

    let env = mock_env();
    let info = mock_info("addr0000", &[]);
    // we can just call .unwrap() to assert this was a success
    let _res = instantiate(deps.as_mut(), env, info, msg).unwrap();

    // store liquidity token
    let reply_msg = Reply {
        id: 1,
        result: SubMsgResult::Ok(SubMsgResponse {
            events: vec![],
            data: Some(
                vec![
                    10, 13, 108, 105, 113, 117, 105, 100, 105, 116, 121, 48, 48, 48, 48,
                ]
                .into(),
            ),
        }),
    };

    let _res = reply(deps.as_mut(), mock_env(), reply_msg).unwrap();

    // unauthorized access; can not execute swap directly for token swap
    let msg = ExecuteMsg::Swap {
        offer_asset: Asset {
            info: AssetInfo::Token {
                contract_addr: "asset0000".to_string(),
            },
            amount: offer_amount,
        },
        ask_asset: AssetInfo::Token {
            contract_addr: "asset0001".to_string(),
        },
        belief_price: None,
        max_spread: None,
        to: None,
    };
    let env = mock_env();
    let info = mock_info("addr0000", &[]);
    let res = execute(deps.as_mut(), env, info, msg).unwrap_err();
    match res {
        ContractError::Unauthorized {} => (),
        _ => panic!("DO NOT ENTER HERE"),
    }

    // normal sell
    let msg = ExecuteMsg::Receive(Cw20ReceiveMsg {
        sender: "addr0000".to_string(),
        amount: offer_amount,
        msg: to_binary(&Cw20HookMsg::Swap {
            ask_asset: AssetInfo::NativeToken {
                denom: "uusd".to_string(),
            },
            belief_price: None,
            max_spread: None,
            to: Some("third_party".to_string()),
        })
        .unwrap(),
    });
    let env = mock_env();
    let info = mock_info("asset0000", &[]);

    let res = execute(deps.as_mut(), env, info, msg).unwrap();
    assert_eq!(res.messages.len(), 2);
    let msg_transfer = res.messages.get(0).expect("no message");

    let expected_spread_amount = Uint128::new(830_233u128);
    //Expected return on a stable swap should about the same as input,
    // swap fee will be approximately 1_500_000_000 * 0.003   4_500_000
    // burn and protocol fees will be approximately 1_500_000_000 * 0.001 1_500_000
    let expected_swap_fee_amount = Uint128::new(4_497_509u128);
    let expected_protocol_fee_amount = Uint128::new(1_499_169u128);
    let expected_burn_fee_amount = Uint128::new(1_499_169u128);
    let expected_return_amount = Uint128::new(1_491_673_920u128);

    // since there is a burn_fee on the PoolFee, check burn message
    // since we swapped to a native token, the burn message should be a BankMsg::Burn
    let expected_burn_msg = SubMsg {
        id: 0,
        msg: CosmosMsg::Bank(BankMsg::Burn {
            amount: coins(expected_burn_fee_amount.u128(), "uusd"),
        }),
        gas_limit: None,
        reply_on: ReplyOn::Never,
    };
    assert_eq!(res.messages.last().unwrap().clone(), expected_burn_msg);

    // as we swapped token to native, we accumulate the protocol fees in native
    let protocol_fees_for_native = query_fees(
        deps.as_ref(),
        Some("uusd".to_string()),
        None,
        COLLECTED_PROTOCOL_FEES,
        Some(ALL_TIME_COLLECTED_PROTOCOL_FEES),
    )
    .unwrap()
    .fees;
    assert_eq!(
        protocol_fees_for_native.first().unwrap().amount,
        expected_protocol_fee_amount
    );
    let protocol_fees_for_token = query_fees(
        deps.as_ref(),
        Some("asset0000".to_string()),
        None,
        COLLECTED_PROTOCOL_FEES,
        Some(ALL_TIME_COLLECTED_PROTOCOL_FEES),
    )
    .unwrap()
    .fees;
    assert_eq!(
        protocol_fees_for_token.first().unwrap().amount,
        Uint128::zero()
    );

    // check simulation res, reset values pre-swap to check simulation
    deps.querier.with_token_balances(&[
        (
            &"liquidity0000".to_string(),
            &[(&MOCK_CONTRACT_ADDR.to_string(), &total_share)],
        ),
        (
            &"asset0000".to_string(),
            &[(&MOCK_CONTRACT_ADDR.to_string(), &(asset_pool_amount))],
        ),
        (
            &"asset0001".to_string(),
            &[(&MOCK_CONTRACT_ADDR.to_string(), &(asset_pool_amount))],
        ),
    ]);

    // reset protocol fees so the simulation returns same values as the actual swap
    COLLECTED_PROTOCOL_FEES
        .save(
            &mut deps.storage,
            &vec![
                Asset {
                    info: AssetInfo::NativeToken {
                        denom: "uusd".to_string(),
                    },
                    amount: Uint128::zero(),
                },
                Asset {
                    info: AssetInfo::Token {
                        contract_addr: "asset0000".to_string(),
                    },
                    amount: Uint128::zero(),
                },
                Asset {
                    info: AssetInfo::Token {
                        contract_addr: "asset0001".to_string(),
                    },
                    amount: Uint128::zero(),
                },
            ],
        )
        .unwrap();

    let simulation_res: SimulationResponse = from_binary(
        &query(
            deps.as_ref(),
            mock_env(),
            QueryMsg::Simulation {
                offer_asset: Asset {
                    info: AssetInfo::Token {
                        contract_addr: "asset0000".to_string(),
                    },
                    amount: offer_amount,
                },
                ask_asset: Asset {
                    info: AssetInfo::NativeToken {
                        denom: "uusd".to_string(),
                    },
                    amount: Default::default(),
                },
            },
        )
        .unwrap(),
    )
    .unwrap();

    assert_eq!(expected_return_amount, simulation_res.return_amount);
    assert_eq!(expected_swap_fee_amount, simulation_res.swap_fee_amount);
    assert_eq!(expected_burn_fee_amount, simulation_res.burn_fee_amount);
    assert_eq!(expected_spread_amount, simulation_res.spread_amount);
    assert_eq!(
        expected_protocol_fee_amount,
        simulation_res.protocol_fee_amount
    );

    // reset protocol fees so the simulation returns same values as the actual swap
    COLLECTED_PROTOCOL_FEES
        .save(
            &mut deps.storage,
            &vec![
                Asset {
                    info: AssetInfo::NativeToken {
                        denom: "uusd".to_string(),
                    },
                    amount: Uint128::zero(),
                },
                Asset {
                    info: AssetInfo::Token {
                        contract_addr: "asset0000".to_string(),
                    },
                    amount: Uint128::zero(),
                },
            ],
        )
        .unwrap();

    // check reverse simulation res
    let reverse_simulation_res: ReverseSimulationResponse = from_binary(
        &query(
            deps.as_ref(),
            mock_env(),
            QueryMsg::ReverseSimulation {
                ask_asset: Asset {
                    info: AssetInfo::NativeToken {
                        denom: "uusd".to_string(),
                    },
                    amount: expected_return_amount,
                },
                offer_asset: Asset {
                    info: AssetInfo::Token {
                        contract_addr: "asset0000".to_string(),
                    },
                    amount: Default::default(),
                },
            },
        )
        .unwrap(),
    )
    .unwrap();

    assert!(
        (offer_amount.u128() as i128 - reverse_simulation_res.offer_amount.u128() as i128).abs()
            < 3i128
    );
    assert!(
        (expected_swap_fee_amount.u128() as i128
            - reverse_simulation_res.swap_fee_amount.u128() as i128)
            .abs()
            < 3i128
    );
    assert!(
        (expected_spread_amount.u128() as i128
            - reverse_simulation_res.spread_amount.u128() as i128)
            .abs()
            < 3i128
    );
    assert!(
        (expected_protocol_fee_amount.u128() as i128
            - reverse_simulation_res.protocol_fee_amount.u128() as i128)
            .abs()
            < 3i128
    );
    assert!(
        (expected_burn_fee_amount.u128() as i128
            - reverse_simulation_res.burn_fee_amount.u128() as i128)
            .abs()
            < 3i128
    );

    assert_eq!(
        res.attributes,
        vec![
            attr("action", "swap"),
            attr("sender", "addr0000"),
            attr("receiver", "third_party"),
            attr("offer_asset", "asset0000"),
            attr("ask_asset", "uusd"),
            attr("offer_amount", offer_amount.to_string()),
            attr("return_amount", expected_return_amount.to_string()),
            attr("spread_amount", expected_spread_amount.to_string()),
            attr("swap_fee_amount", expected_swap_fee_amount.to_string()),
            attr(
                "protocol_fee_amount",
                expected_protocol_fee_amount.to_string(),
            ),
            attr("burn_fee_amount", expected_burn_fee_amount.to_string(),)
        ]
    );

    assert_eq!(
        &SubMsg::new(CosmosMsg::Bank(BankMsg::Send {
            to_address: "third_party".to_string(),
            amount: vec![Coin {
                denom: "uusd".to_string(),
                amount: expected_return_amount,
            }],
        })),
        msg_transfer,
    );

    // failed due to non asset token contract try to execute sell
    let msg = ExecuteMsg::Receive(Cw20ReceiveMsg {
        sender: "addr0000".to_string(),
        amount: offer_amount,
        msg: to_binary(&Cw20HookMsg::Swap {
            ask_asset: AssetInfo::Token {
                contract_addr: "asset0000".to_string(),
            },
            belief_price: None,
            max_spread: None,
            to: None,
        })
        .unwrap(),
    });
    let env = mock_env();
    let info = mock_info("liquidity0000", &[]);
    let res = execute(deps.as_mut(), env, info, msg).unwrap_err();
    match res {
        ContractError::Unauthorized {} => (),
        _ => panic!("DO NOT ENTER HERE"),
    }
}

#[test]
fn test_swap_to_third_party() {
    let total_share = Uint128::from(30_000_000_000u128);
    let asset_pool_amount = Uint128::from(20_000_000_000u128);
    let collateral_pool_amount = Uint128::from(30_000_000_000u128);
    let offer_amount = Uint128::from(1_500_000_000u128);

    let mut deps = mock_dependencies(&[Coin {
        denom: "uusd".to_string(),
        amount: collateral_pool_amount + offer_amount,
        /* user deposit must be pre-applied */
    }]);

    deps.querier.with_token_balances(&[
        (
            &"liquidity0000".to_string(),
            &[(&MOCK_CONTRACT_ADDR.to_string(), &total_share)],
        ),
        (
            &"asset0000".to_string(),
            &[(&MOCK_CONTRACT_ADDR.to_string(), &asset_pool_amount)],
        ),
        (
            &"asset0001".to_string(),
            &[(&MOCK_CONTRACT_ADDR.to_string(), &asset_pool_amount)],
        ),
    ]);

    let msg = InstantiateMsg {
        asset_infos: [
            AssetInfo::NativeToken {
                denom: "uusd".to_string(),
            },
            AssetInfo::Token {
                contract_addr: "asset0000".to_string(),
            },
            AssetInfo::Token {
                contract_addr: "asset0001".to_string(),
            },
        ],
        token_code_id: 10u64,
        asset_decimals: [6u8, 8u8, 10u8],
        pool_fees: PoolFee {
            protocol_fee: Fee {
                share: Decimal::from_ratio(1u128, 1000u128),
            },
            swap_fee: Fee {
                share: Decimal::from_ratio(3u128, 1000u128),
            },
            burn_fee: Fee {
                share: Decimal::zero(),
            },
        },
        fee_collector_addr: "collector".to_string(),
        amp_factor: 1000,
    };

    let env = mock_env();
    let info = mock_info("addr0000", &[]);
    instantiate(deps.as_mut(), env, info, msg).unwrap();

    // store liquidity token
    let reply_msg = Reply {
        id: 1,
        result: SubMsgResult::Ok(SubMsgResponse {
            events: vec![],
            data: Some(
                vec![
                    10, 13, 108, 105, 113, 117, 105, 100, 105, 116, 121, 48, 48, 48, 48,
                ]
                .into(),
            ),
        }),
    };

    reply(deps.as_mut(), mock_env(), reply_msg).unwrap();

    // first swap
    let msg = ExecuteMsg::Swap {
        offer_asset: Asset {
            info: AssetInfo::NativeToken {
                denom: "uusd".to_string(),
            },
            amount: offer_amount,
        },
        ask_asset: AssetInfo::Token {
            contract_addr: "asset0000".to_string(),
        },
        belief_price: None,
        max_spread: None,
        to: Some("third_party".to_string()),
    };
    let env = mock_env();
    let info = mock_info(
        "addr0000",
        &[Coin {
            denom: "uusd".to_string(),
            amount: offer_amount,
        }],
    );
    let res = execute(deps.as_mut(), env, info, msg).unwrap();
    assert_eq!(res.messages.len(), 1); //no burn msg as there is no burn_fee

    assert_eq!(
        res.attributes
            .iter()
            .find(|&a| a.key == "receiver")
            .map(|a| a.clone().value)
            .unwrap(),
        "third_party"
    );

    // reset protocol fees so the simulation returns same values as the actual swap
    COLLECTED_PROTOCOL_FEES
        .save(
            &mut deps.storage,
            &vec![
                Asset {
                    info: AssetInfo::NativeToken {
                        denom: "uusd".to_string(),
                    },
                    amount: Uint128::zero(),
                },
                Asset {
                    info: AssetInfo::Token {
                        contract_addr: "asset0000".to_string(),
                    },
                    amount: Uint128::zero(),
                },
            ],
        )
        .unwrap();

    let simulation_res: SimulationResponse = from_binary(
        &query(
            deps.as_ref(),
            mock_env(),
            QueryMsg::Simulation {
                offer_asset: Asset {
                    info: AssetInfo::NativeToken {
                        denom: "uusd".to_string(),
                    },
                    amount: offer_amount,
                },
                ask_asset: Asset {
                    info: AssetInfo::Token {
                        contract_addr: "asset0000".to_string(),
                    },
                    amount: Default::default(),
                },
            },
        )
        .unwrap(),
    )
    .unwrap();

    // there shouldn't be any burn_fee
    assert_eq!(simulation_res.burn_fee_amount, Uint128::zero());
}
