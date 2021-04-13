//! This contract implements [the SNIP-20 standard](https://github.com/SecretFoundation/SNIPs/blob/master/SNIP-20.md)

#[macro_use] extern crate fadroma;
pub mod receiver; use crate::receiver::Snip20ReceiveMsg;
pub mod state; use crate::state::{
    get_receiver_hash, get_transfers, read_allowance, read_viewing_key, set_receiver_hash,
    store_transfer, write_allowance, write_viewing_key, Balances, Config, Constants,
    ReadonlyBalances, ReadonlyConfig,
};
pub mod msg; use crate::msg::{
    InitialBalance, ResponseStatus, ContractStatusLevel,
    QueryAnswer, HandleAnswer, space_pad
};
mod rand; use crate::rand::sha_256;
mod utils;
mod viewing_key; use crate::viewing_key::{ViewingKey, VIEWING_KEY_SIZE};

use cosmwasm_std::{
    log, to_binary, Api, Binary, CanonicalAddr, CosmosMsg, Env, Extern,
    HandleResponse, HumanAddr, InitResponse, Querier, QueryResult, ReadonlyStorage, StdError,
    StdResult, Storage, Uint128,
};

contract!(
    [State] {}
    [Init] (deps, env, msg: {
        name:             String,
        admin:            Option<HumanAddr>,
        symbol:           String,
        decimals:         u8,
        initial_balances: Option<Vec<InitialBalance>>,
        prng_seed:        Binary,
        config:           Option<InitConfig>
    }) {
        validate_basics(&name, &symbol, decimals)?;
        let init_config  = msg.config();
        let mut config = Config::from_storage(&mut deps.storage);
        config.set_constants(&Constants {
            name:                   name,
            symbol:                 symbol,
            decimals:               decimals,
            admin:                  admin.unwrap_or_else(|| env.message.sender).clone(),
            prng_seed:              sha_256(&prng_seed.0).to_vec(),
            total_supply_is_public: init_config.public_total_supply(),
        })?;
        config.set_total_supply(initial_total_supply(deps, &initial_balances.unwrap_or_default()));
        config.set_contract_status(ContractStatusLevel::NormalRun);
        config.set_minters(Vec::from([admin]))?;
        Ok(InitResponse::default())
    }
    [Query] (deps, state, msg) {
        TokenInfo () {
            let config = ReadonlyConfig::from_storage(storage);
            let constants = config.constants()?;
            to_binary(&QueryAnswer::TokenInfo {
                name:     constants.name,
                symbol:   constants.symbol,
                decimals: constants.decimals,
                total_supply: if constants.total_supply_is_public {
                    Some(Uint128(config.total_supply()))
                } else {
                    None
                },
            })
        }
        /// FIXME: Returns a constant 1:1 rate to uscrt, since that's the purpose of this
        ExchangeRate () {
            to_binary(&QueryAnswer::ExchangeRate {
                rate: Uint128(1),
                denom: "uscrt".to_string(),
            })
        }
        /// Initially, the admin is the only minter.
        Minters () {
            let minters  = ReadonlyConfig::from_storage(&deps.storage).minters();
            let response = QueryAnswer::Minters { minters };
            to_binary(&response)
        }
        /// Requires viewing key
        Balance (address: HumanAddr, key: String) {
            let address  = deps.api.canonical_address(account)?;
            let amount   = Uint128(ReadonlyBalances::from_storage(&deps.storage).account_amount(&address));
            to_binary(&QueryAnswer::Balance { amount })
        }
        /// Requires viewing key
        TransferHistory (address: HumanAddr, key: String, page: u32, page_size: u32) {
            let address = deps.api.canonical_address(account).unwrap();
            let txs    = get_transfers(&deps.api, &deps.storage, &address, page, page_size)?;
            to_binary(&QueryAnswer::TransferHistory { txs })
        }
        /// Requires viewing key
        Allowance (owner: HumanAddr, spender: HumanAddr, key: String) {
            try_check_allowance(deps, owner, spender);
            let owner_canon   = deps.api.canonical_address(&owner)?;
            let spender_canon = deps.api.canonical_address(&spender)?;
            let allowance  = read_allowance(&deps.storage, &owner_canon, &spender_canon)?;
            let expiration = allowance.expiration;
            let allowance  = Uint128(allowance.amount);
            to_binary(&QueryAnswer::Allowance { owner, spender, allowance, expiration })
        }
    }
    [Response] {}
    [Handle] (deps, env, state, msg) {
        /// Allows transactions to be stopped 
        SetContractStatus (level: ContractStatusLevel) {
            let mut config = Config::from_storage(&mut deps.storage);
            is_admin(&config, &env.message.sender)?;

            config.set_contract_status(level);
            let data = Some(to_binary(&HandleAnswer::SetContractStatus { status: ResponseStatus::Success, })?);
            Ok((padded(HandleResponse { messages: vec![], log: vec![], data }), None))
        }
        /// Set the admin
        ChangeAdmin (address: HumanAddr) {
            let mut config = Config::from_storage(&mut deps.storage);
            is_not_stopped(&config)?;
            is_admin(&config, &env.message.sender)?;

            let mut consts = config.constants()?;
            consts.admin = address;
            config.set_constants(&consts)?;
            let data = Some(to_binary(&HandleAnswer::ChangeAdmin { status: ResponseStatus::Success })?);
            Ok((padded(HandleResponse { messages: vec![], log: vec![], data }), None))
        }
        /// Set minters
        SetMinters (minters_to_set: Vec<HumanAddr>) {
            let mut config = Config::from_storage(&mut deps.storage);
            is_not_stopped(&config)?;
            is_admin(&config, &env.message.sender)?;

            config.set_minters(minters_to_set)?;
            let data = Some(to_binary(&HandleAnswer::SetMinters { status: ResponseStatus::Success })?);
            Ok((padded(HandleResponse { messages: vec![], log: vec![], data }), None))
        }
        /// Add minters
        AddMinters (minters_to_add: Vec<HumanAddr>) {
            let mut config = Config::from_storage(&mut deps.storage);
            is_not_stopped(&config)?;
            is_admin(&config, &env.message.sender)?;

            config.add_minters(minters_to_add)?;
            let data = Some(to_binary(&HandleAnswer::AddMinters { status: ResponseStatus::Success })?);
            Ok((padded(HandleResponse { messages: vec![], log: vec![], data }), None))
        }
        /// Remove minters
        RemoveMinters (minters: Vec<HumanAddr>) {
            let mut config = Config::from_storage(&mut deps.storage);
            is_not_stopped(&config)?;
            is_admin(&config, &env.message.sender)?;

            config.remove_minters(minters_to_remove)?;
            let data = Some(to_binary(&HandleAnswer::RemoveMinters { status: Success })?)
            padded(Ok(HandleResponse { messages: vec![], log: vec![], data }))
        }
        Mint (recipient: HumanAddr, amount: Uin128) {
            let mut config = Config::from_storage(&mut deps.storage);
            is_not_stopped(&config)?;
            is_minter(&config, &env.message.sender);

            let amount = amount.u128();
            let mut total_supply = config.total_supply();
            if let Some(new_total_supply) = total_supply.checked_add(amount) {
                total_supply = new_total_supply;
            } else {
                return Err(StdError::generic_err(
                    "This mint attempt would increase the total supply above the supported maximum",
                ));
            }
            config.set_total_supply(total_supply);
            let receipient_account = &deps.api.canonical_address(&address)?;
            let mut balances = Balances::from_storage(&mut deps.storage);
            let mut account_balance = balances.balance(receipient_account);
            if let Some(new_balance) = account_balance.checked_add(amount) {
                account_balance = new_balance;
            } else {
                // This error literally can not happen, since the account's funds are a subset
                // of the total supply, both are stored as u128, and we check for overflow of
                // the total supply just a couple lines before.
                // Still, writing this to cover all overflows.
                return Err(StdError::generic_err(
                    "This mint attempt would increase the account's balance above the supported maximum",
                ));
            }
            balances.set_account_balance(receipient_account, account_balance);
            let data = Some(to_binary(&HandleAnswer::Mint { status: Success })?);
            padded(Ok(HandleResponse { messages: vec![], log: vec![], data }))
        }
        /// Remove `amount` tokens from the system irreversibly, from signer account
        /// @param amount the amount of money to burn
        Burn (amount: Uint128) {
            let mut config = Config::from_storage(&mut deps.storage);
            is_not_stopped(&config);

            let sender = deps.api.canonical_address(&env.message.sender)?;
            let amount = amount.u128();
            let mut balances = Balances::from_storage(&mut deps.storage);
            let mut account_balance = balances.balance(&sender);
            if let Some(new_account_balance) = account_balance.checked_sub(amount) {
                account_balance = new_account_balance;
            } else {
                return Err(StdError::generic_err(format!(
                    "insufficient funds to burn: balance={}, required={}",
                    account_balance, amount
                )));
            }
            balances.set_account_balance(&sender, account_balance);
            let mut config = Config::from_storage(&mut deps.storage);
            let mut total_supply = config.total_supply();
            if let Some(new_total_supply) = total_supply.checked_sub(amount) {
                total_supply = new_total_supply;
            } else {
                return Err(StdError::generic_err(
                    "You're trying to burn more than is available in the total supply",
                ));
            }
            config.set_total_supply(total_supply);

            let data = Some(to_binary(&HandleAnswer::Burn { status: Success })?);
            padded(Ok(HandleResponse { messages: vec![], log: vec![], data }))
        }
        BurnFrom (owner: HumanAddr, amount: Uint128) {
            let mut config = Config::from_storage(&mut deps.storage);
            is_not_stopped(&config);

            let spender = deps.api.canonical_address(&env.message.sender)?;
            let owner = deps.api.canonical_address(owner)?;
            let amount = amount.u128();
            let mut allowance = read_allowance(&deps.storage, &owner, &spender)?;
            if allowance.expiration.map(|ex| ex < env.block.time) == Some(true) {
                allowance.amount = 0;
                write_allowance(&mut deps.storage, &owner, &spender, allowance)?;
                return Err(insufficient_allowance(0, amount));
            }
            if let Some(new_allowance) = allowance.amount.checked_sub(amount) {
                allowance.amount = new_allowance;
            } else {
                return Err(insufficient_allowance(allowance.amount, amount));
            }
            write_allowance(&mut deps.storage, &owner, &spender, allowance)?;
            // subtract from owner account
            let mut balances = Balances::from_storage(&mut deps.storage);
            let mut account_balance = balances.balance(&owner);
            if let Some(new_balance) = account_balance.checked_sub(amount) {
                account_balance = new_balance;
            } else {
                return Err(StdError::generic_err(format!(
                    "insufficient funds to burn: balance={}, required={}",
                    account_balance, amount
                )));
            }
            balances.set_account_balance(&owner, account_balance);
            // remove from supply
            let mut config = Config::from_storage(&mut deps.storage);
            if let Some(new_total_supply) = total_supply.checked_sub(amount) {
                config.set_total_supply(new_total_supply);
            } else {
                return Err(StdError::generic_err(
                    "You're trying to burn more than is available in the total supply",
                ));
            }
            let data = Some(to_binary(&HandleAnswer::BurnFrom { status: Success })?);
            padded(Ok(HandleResponse { messages: vec![], log: vec![], data }))
        }

        Deposit () {
            Err((StdError::generic_err("not allowed."), None))
        }
        Redeem () {
            Err((StdError::generic_err("not allowed."), None))
        }

        Transfer (recipient: HumanAddr, amount: Uint128) {
            let mut config = Config::from_storage(&mut deps.storage);
            is_not_stopped(&config)?;

            let response = try_transfer(deps, env, &recipient, amount)?;

            Ok((padded(response), None)
        }
        TransferFrom (owner: HumanAddr, recipient: HumanAddr, amount: Uint128) {
            let mut config = Config::from_storage(&mut deps.storage);
            is_not_stopped(&config)?;

            try_transfer_from_impl(deps, env, owner, recipient, amount)?;

            let data = Some(to_binary(&HandleAnswer::TransferFrom { status: ResponseStatus::Success })?)
            Ok((padded(HandleResponse { messages: vec![], log: vec![], data }), None))
        }
        Send (recipient: HumanAddr, amount: Uint128, msg: Option<Binary>) {
            let mut config = Config::from_storage(&mut deps.storage);
            is_not_stopped(&config)?;

            let response = try_send(deps, env, &recipient, amount, msg)?;

            Ok((padded(response), None)
        }
        SendFrom (owner: HumanAddr, recipient: HumanAddr, amount: Uint128, msg: Option<Binary>) {
            let mut config = Config::from_storage(&mut deps.storage);
            is_not_stopped(&config)?;

            let sender = env.message.sender.clone();
            try_transfer_from_impl(deps, env, owner, recipient, amount)?;

            let mut messages = vec![];
            try_add_receiver_api_callback(&mut messages, &deps.storage, recipient, msg, sender, owner.clone(), amount)?;

            let data = Some(to_binary(&HandleAnswer::SendFrom { status: ResponseStatus::Success })?);
            Ok((padded(HandleResponse { messages, log: vec![], data }), None))
        }

        CreateViewingKey (entropy: String) {
            let mut config = Config::from_storage(&mut deps.storage);
            is_not_stopped(&config)?;

            let constants = ReadonlyConfig::from_storage(&deps.storage).constants()?;
            let prng_seed = constants.prng_seed;
            let key       = ViewingKey::new(&env, &prng_seed, (&entropy).as_ref());
            let sender    = deps.api.canonical_address(&env.message.sender)?;
            write_viewing_key(&mut deps.storage, &sender, &key);

            let data = Some(to_binary(&HandleAnswer::CreateViewingKey { key })?);
            Ok((padded(HandleResponse { messages: vec![], log: vec![], data }), None))
        }
        SetViewingKey (key: String) {
            let mut config = Config::from_storage(&mut deps.storage);
            is_not_stopped(&config)?;

            let vk     = ViewingKey(key);
            let sender = deps.api.canonical_address(&env.message.sender)?;
            write_viewing_key(&mut deps.storage, &sender, &vk);

            let data = Some(to_binary(&HandleAnswer::SetViewingKey { status: ResponseStatus::Success })?);
            Ok((padded(HandleResponse { messages: vec![], log: vec![], data }), None))
        }

        RegisterReceive (code_hash: String) {
            let mut config = Config::from_storage(&mut deps.storage);
            is_not_stopped(&config)?;

            set_receiver_hash(&mut deps.storage, &env.message.sender, code_hash);

            let log  = vec![log("register_status", "success")];
            let msg  = HandleAnswer::RegisterReceive { status: ResponseStatus::Success };
            let data = Some(to_binary(&msg)?);
            Ok((padded(HandleResponse { messages: vec![], log, data }), None))
        }

        IncreaseAllowance (spender: HumanAddr, amount: Uint128, expiration: Option<u64>) {
            let mut config = Config::from_storage(&mut deps.storage);
            is_not_stopped(&config);

            let owner_canon = deps.api.canonical_address(&env.message.sender)?;
            let spender_canon = deps.api.canonical_address(&spender)?;
            let mut allowance = read_allowance(&deps.storage, &owner_canon, &spender_canon)?;
            allowance.amount = allowance.amount.saturating_add(amount.u128());
            if expiration.is_some() { allowance.expiration = expiration; }
            let new_amount = allowance.amount;
            write_allowance(&mut deps.storage, &owner, &spender, allowance)?;

            let owner = env.message.sender;
            let allowance = Uint128(new_amount);
            let msg = HandleAnswer::IncreaseAllowance { owner, spender, allowance };
            let data = Some(to_binary(&msg)?);
            Ok((padded(HandleResponse { messages: vec![], log: vec![], data }), None))
        }
        DecreaseAllowance (spender: HumanAddr, amount: Uint128, expiration: Option<String>) {
            let mut config = Config::from_storage(&mut deps.storage);
            is_not_stopped(&config);

            let owner_canon = deps.api.canonical_address(&env.message.sender)?;
            let spender_canon = deps.api.canonical_address(&spender)?;
            let mut allowance = read_allowance(&deps.storage, &owner_canon, &spender_canon)?;
            allowance.amount = allowance.amount.saturating_sub(amount.u128());
            if expiration.is_some() { allowance.expiration = expiration; }
            let new_amount = allowance.amount;
            write_allowance(&mut deps.storage, &owner, &spender, allowance)?;

            let owner = env.message.sender;
            let allowance = Uint128(new_amount);
            let msg = HandleAnswer::DecreaseAllowance { owner, spender, allowance };
            let data = Some(to_binary(&msg)?);
            Ok((padded(HandleResponse { messages: vec![], log: vec![], data }), None))
        }
    }
);

/// We make sure that responses from `handle` are padded to a multiple of this size.
pub const RESPONSE_BLOCK_SIZE: usize = 256;

fn initial_total_supply <S:Storage,A:Api,Q:Querier> (
    deps:             &mut Extern<S, A, Q>,
    initial_balances: Vec<InitialBalance>
) -> StdResult<u128> {
    let mut total_supply: u128 = 0;
    let mut balances = Balances::from_storage(&mut deps.storage);
    for balance in initial_balances {
        let balance_canon = deps.api.canonical_address(&balance.address)?;
        let amount = balance.amount.u128();
        balances.set_account_balance(&balance_canon, amount);
        if let Some(new_total_supply) = total_supply.checked_add(amount) {
            total_supply = new_total_supply;
        } else {
            return Err(StdError::generic_err(
                "The sum of all initial balances exceeds the maximum possible total supply",
            ));
        }
    }
    Ok(total_supply)
}

fn padded (response: HandleResponse) -> HandleResponse {
    response.data = response.data.map(|mut data| {
        space_pad(RESPONSE_BLOCK_SIZE, &mut data.0);
        data
    });
    response
}

fn is_not_stopped <'a,S:Storage> (config: &Config<'a,S>) -> StdResult<()> {
    let contract_status = config.contract_status();
    match contract_status {
        ContractStatusLevel::NormalRun =>
            {}, // If it's a normal run just continue
        ContractStatusLevel::StopAllButRedeems =>
            { unimplemented!() },
        ContractStatusLevel::StopAll =>
            return Err(StdError::generic_err(
                "This contract is stopped and this action is not allowed",
            ))
    }
    Ok(())
}

fn try_transfer_impl<S: Storage, A: Api, Q: Querier>(
    deps: &mut Extern<S, A, Q>,
    env: Env,
    recipient: &HumanAddr,
    amount: Uint128,
) -> StdResult<()> {
    let sender = deps.api.canonical_address(&env.message.sender)?;
    let recipient = deps.api.canonical_address(recipient)?;

    perform_transfer(
        &mut deps.storage,
        &sender,
        &recipient,
        amount.u128(),
    )?;

    let symbol = Config::from_storage(&mut deps.storage).constants()?.symbol;

    store_transfer(
        &mut deps.storage,
        &sender,
        &sender,
        &recipient,
        amount,
        symbol,
    )?;

    Ok(())
}


fn try_add_receiver_api_callback<S: ReadonlyStorage>(
    messages: &mut Vec<CosmosMsg>,
    storage: &S,
    recipient: &HumanAddr,
    msg: Option<Binary>,
    sender: HumanAddr,
    from: HumanAddr,
    amount: Uint128,
) -> StdResult<()> {
    let receiver_hash = get_receiver_hash(storage, recipient);
    if let Some(receiver_hash) = receiver_hash {
        let receiver_hash = receiver_hash?;
        let receiver_msg = Snip20ReceiveMsg::new(sender, from, amount, msg);
        let callback_msg = receiver_msg.into_cosmos_msg(receiver_hash, recipient.clone())?;

        messages.push(callback_msg);
    }
    Ok(())
}

fn try_send<S: Storage, A: Api, Q: Querier>(
    deps: &mut Extern<S, A, Q>,
    env: Env,
    recipient: &HumanAddr,
    amount: Uint128,
    msg: Option<Binary>,
) -> StdResult<HandleResponse> {
    let sender = env.message.sender.clone();
    try_transfer_impl(deps, env, recipient, amount)?;
    let mut messages = vec![];
    try_add_receiver_api_callback(
        &mut messages,
        &deps.storage,
        recipient,
        msg,
        sender.clone(),
        sender,
        amount,
    )?;
    let data = Some(to_binary(&HandleAnswer::Send { status: ResponseStatus::Success })?);
    Ok(HandleResponse { messages, log: vec![], data })
}

fn insufficient_allowance(allowance: u128, required: u128) -> StdError {
    StdError::generic_err(format!(
        "insufficient allowance: allowance={}, required={}",
        allowance, required
    ))
}

fn try_transfer_from_impl<S: Storage, A: Api, Q: Querier>(
    deps: &mut Extern<S, A, Q>,
    env: Env,
    owner: &HumanAddr,
    recipient: &HumanAddr,
    amount: Uint128,
) -> StdResult<()> {
    let spender    = deps.api.canonical_address(&env.message.sender)?;
    let owner      = deps.api.canonical_address(owner)?;
    let recipient  = deps.api.canonical_address(recipient)?;
    let amount_raw = amount.u128();
    let mut allowance = read_allowance(&deps.storage, &owner, &spender)?;
    if allowance.expiration.map(|ex| ex < env.block.time) == Some(true) {
        allowance.amount = 0;
        write_allowance(&mut deps.storage, &owner, &spender, allowance)?;
        return Err(insufficient_allowance(0, amount_raw));
    }
    if let Some(new_allowance) = allowance.amount.checked_sub(amount_raw) {
        allowance.amount = new_allowance;
    } else {
        return Err(insufficient_allowance(allowance.amount, amount_raw));
    }
    write_allowance(&mut deps.storage, &owner, &spender, allowance,)?;
    perform_transfer(&mut deps.storage, &owner, &recipient, amount_raw)?;
    let symbol = Config::from_storage(&mut deps.storage).constants()?.symbol;
    store_transfer(&mut deps.storage, &owner, &spender, &recipient, amount, symbol,)?;
    Ok(())
}

fn try_transfer_from<S: Storage, A: Api, Q: Querier>(
    deps: &mut Extern<S, A, Q>,
    env: Env,
    owner: &HumanAddr,
    recipient: &HumanAddr,
    amount: Uint128,
) -> StdResult<HandleResponse> {
    try_transfer_from_impl(deps, env, owner, recipient, amount)?;
    let data = Some(to_binary(&HandleAnswer::TransferFrom { status: ResponseStatus::Success })?);
    Ok(HandleResponse { messages: vec![], log: vec![], data })
}

fn perform_transfer<T: Storage>(
    store: &mut T,
    from: &CanonicalAddr,
    to: &CanonicalAddr,
    amount: u128,
) -> StdResult<()> {
    let mut balances = Balances::from_storage(store);

    let mut from_balance = balances.balance(from);
    if let Some(new_from_balance) = from_balance.checked_sub(amount) {
        from_balance = new_from_balance;
    } else {
        return Err(StdError::generic_err(format!(
            "insufficient funds: balance={}, required={}",
            from_balance, amount
        )));
    }
    balances.set_account_balance(from, from_balance);

    let mut to_balance = balances.balance(to);
    to_balance = to_balance.checked_add(amount).ok_or_else(|| {
        StdError::generic_err("This tx will literally make them too rich. Try transferring less")
    })?;
    balances.set_account_balance(to, to_balance);

    Ok(())
}

fn is_admin<S: Storage>(config: &Config<S>, account: &HumanAddr) -> StdResult<()> {
    let consts = config.constants()?;
    if &consts.admin != account {
        return Err(StdError::generic_err("This is an admin command. Admin commands can only be run from admin address",));
    }
    Ok(())
}

fn is_minter <S:Storage> (config: &Config<S>, account: &HumanAddr) -> StdResult<()> {
    let minters = config.minters();
    if !minters.contains(account) {
        return Err(StdError::generic_err("Minting is allowed to minter accounts only"));
    }
    Ok(())
}

fn validate_basics (name: &str, symbol: &str, decimals: u8) -> StdResult<()> {
    // Check name, symbol, decimals
    if !is_valid_name(&name) {
        return Err(StdError::generic_err("Name is not in the expected format (3-30 UTF-8 bytes)",));
    }
    if !is_valid_symbol(&symbol) {
        return Err(StdError::generic_err("Ticker symbol is not in expected format [A-Z]{3,6}",));
    }
    if decimals > 18 {
        return Err(StdError::generic_err("Decimals must not exceed 18"));
    }
    Ok(())
}

fn is_valid_name(name: &str) -> bool {
    let len = name.len();
    3 <= len && len <= 30
}

fn is_valid_symbol(symbol: &str) -> bool {
    let len = symbol.len();
    let len_is_valid = 3 <= len && len <= 6;

    len_is_valid && symbol.bytes().all(|byte| b'A' <= byte && byte <= b'Z')
}

// pub fn migrate<S: Storage, A: Api, Q: Querier>(
//     _deps: &mut Extern<S, A, Q>,
//     _env: Env,
//     _msg: MigrateMsg,
// ) -> StdResult<MigrateResponse> {
//     Ok(MigrateResponse::default())
// }

pub fn try_check_allowance<S: Storage, A: Api, Q: Querier>(
    deps: &Extern<S, A, Q>,
    owner: HumanAddr,
    spender: HumanAddr,
) -> StdResult<Binary> {
    let owner_canon = deps.api.canonical_address(&owner)?;
    let spender_canon = deps.api.canonical_address(&spender)?;
    let allowance = read_allowance(&deps.storage, &owner_canon, &spender_canon)?;
    to_binary(&QueryAnswer::Allowance {
        owner,
        spender,
        allowance: Uint128(allowance.amount),
        expiration: allowance.expiration,
    })
}
