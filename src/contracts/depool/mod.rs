use std::collections::BTreeMap;
use std::sync::Arc;

use anyhow::{Context, Result};
use nekoton_abi::{
    BuildTokenValue, FunctionBuilder, FunctionExt, KnownParamType, KnownParamTypePlain,
    PackAbiPlain, TokenValueExt, UnpackAbi, UnpackAbiPlain, UnpackFirst,
};
use nekoton_utils::SimpleClock;
use num::ToPrimitive;
use ton_abi::contract::ABI_VERSION_2_2;
use ton_block::{Deserializable, Serializable};

use super::{InternalMessage, ONE_EVER};
use crate::config::DePoolType;
use crate::network::Subscription;
use crate::util::make_default_headers;

#[derive(Debug, Clone)]
pub struct DePoolInitParams {
    pub min_stake: u64,
    pub validator_assurance: u64,
    pub owner: ton_block::MsgAddressInt,
    pub participant_reward_fraction: u8,
}

pub struct DePool {
    ty: DePoolType,
    keypair: Option<ed25519_dalek::Keypair>,
    address: ton_block::MsgAddressInt,
    subscription: Arc<Subscription>,
}

impl DePool {
    pub const INITIAL_BALANCE: u128 = 30 * ONE_EVER;
    pub const CRITICAL_BALANCE: u128 = 20 * ONE_EVER;
    pub const INITIAL_PROXY_BALANCE: u128 = 3 * ONE_EVER;
    pub const MIN_PROXY_BALANCE: u128 = 2 * ONE_EVER;

    pub fn new(
        ty: DePoolType,
        address: ton_block::MsgAddressInt,
        subscription: Arc<Subscription>,
    ) -> Self {
        Self {
            ty,
            keypair: None,
            address,
            subscription,
        }
    }

    pub fn address(&self) -> &ton_block::MsgAddressInt {
        &self.address
    }

    pub fn set_keypair(&mut self, keypair: ed25519_dalek::Keypair) -> Result<()> {
        let computed_address = self.ty.compute_depool_address(&keypair.public)?;
        anyhow::ensure!(
            computed_address == self.address,
            "wrong DePool address or keys"
        );
        self.keypair = Some(keypair);
        Ok(())
    }

    pub async fn is_deployed(&self) -> Result<bool> {
        let Some(account) = self
            .subscription
            .get_account_state(&self.address)
            .await
            .context("failed to get DePool state")? else {
            return Ok(false)
        };

        match account.storage.state {
            ton_block::AccountState::AccountActive { .. } => Ok(true),
            ton_block::AccountState::AccountFrozen { .. } => anyhow::bail!("account frozen"),
            ton_block::AccountState::AccountUninit => Ok(false),
        }
    }

    pub async fn get_balance(&self) -> Result<Option<u128>> {
        let account = self.subscription.get_account_state(&self.address).await?;
        Ok(account.map(|state| state.storage.balance.grams.0))
    }

    pub async fn deploy(&self, params: DePoolInitParams) -> Result<()> {
        let keypair = self.keypair.as_ref().context("DePool keypair not set")?;

        let inputs = ConstructorInputs {
            min_stake: params.min_stake,
            validator_assurance: params.validator_assurance,
            validator_wallet: params.owner,
            proxy_code: self.ty.proxy_code().clone(),
            participant_reward_fraction: params.participant_reward_fraction,
        }
        .pack();

        self.subscription
            .send_message_with_retires(move |timeout| {
                let (expire_at, header) = make_default_headers(None, timeout);

                let mut message = self.external_message_to_self(
                    common::constructor()
                        .encode_input(
                            &header,
                            &inputs,
                            false,
                            Some(keypair),
                            Some(self.address.clone()),
                        )
                        .context("failed to encode constructor")?,
                );

                message.set_state_init(self.ty.compute_depool_state_init(&keypair.public)?);

                Ok((message, expire_at))
            })
            .await
            .context("failed to send constructor message")?;

        Ok(())
    }

    pub fn ticktock(&self) -> Result<InternalMessage> {
        Ok(self.internal_message_to_self(ONE_EVER, common::ticktock().encode_internal_input(&[])?))
    }

    pub async fn maintain_balances(&self) -> Result<Vec<InternalMessage>> {
        let account = self
            .subscription
            .get_account_state(&self.address)
            .await?
            .context("DePool not deployed")?;

        let mut messages = Vec::new();

        let depool_info = self.get_info(&account)?;

        // Check depool balance
        let depool_balance = {
            let tokens = self.run_local(&account, common::get_depool_balance(), &[])?;
            match tokens.into_iter().next() {
                Some(ton_abi::Token {
                    value: ton_abi::TokenValue::Int(ton_abi::Int { number, .. }),
                    ..
                }) => number,
                _ => return Err(nekoton_abi::UnpackerError::InvalidAbi.into()),
            }
        };

        let critical_balance = num::BigInt::from(Self::CRITICAL_BALANCE);
        if depool_balance <= critical_balance {
            let remaining = num::BigInt::from(Self::INITIAL_BALANCE) - depool_balance;
            if let Some(remaining) = remaining.to_u128() {
                messages.push(self.internal_message_to_self(
                    remaining,
                    &common::receive_funds().encode_internal_input(&[])?,
                ));
            }
        }

        // Check proxies
        for proxy in depool_info.proxies {
            let account = self
                .subscription
                .get_account_state(&proxy)
                .await
                .context("failed to get proxy state")?
                .context("proxy not deployed")?;

            let proxy_balance = match account.storage.state {
                ton_block::AccountState::AccountActive { .. } => account.storage.balance.grams.0,
                ton_block::AccountState::AccountFrozen { .. } => {
                    anyhow::bail!("proxy {proxy} frozen");
                }
                ton_block::AccountState::AccountUninit => {
                    anyhow::bail!("proxy {proxy} not deployed");
                }
            };

            if proxy_balance <= Self::MIN_PROXY_BALANCE {
                messages.push(InternalMessage {
                    amount: Self::INITIAL_PROXY_BALANCE - proxy_balance,
                    dst: proxy,
                    payload: Default::default(),
                });
            }
        }

        Ok(messages)
    }

    pub fn add_ordinary_stake(&self, amount: u64) -> Result<InternalMessage> {
        Ok(self.internal_message_to_self(
            (amount as u128) + ONE_EVER / 2,
            &common::add_ordinary_stake()
                .encode_internal_input(&[amount.token_value().named("stake")])?,
        ))
    }

    pub fn set_allowed_participant(
        &self,
        address: &ton_block::MsgAddressInt,
    ) -> Result<InternalMessage> {
        self.ensure_stever()?;
        Ok(self.internal_message_to_self(
            ONE_EVER,
            stever_v1::set_allowed_participant()
                .encode_internal_input(&[address.clone().token_value().named("addr")])?,
        ))
    }

    pub fn get_participant_info(
        &self,
        state: &ton_block::AccountStuff,
        addr: &ton_block::MsgAddressInt,
    ) -> Result<Option<ParticipantInfo>> {
        const ERR_NOT_PARTICIPANT: i32 = 116;

        let result = match self.ty {
            DePoolType::DefaultV3 => common::get_participant_info(),
            DePoolType::StEver => stever_v1::get_participant_info(),
        }
        .run_local(
            &SimpleClock,
            state.clone(),
            &[addr.clone().token_value().named("addr")],
        )?;

        if result.result_code == ERR_NOT_PARTICIPANT {
            Ok(None)
        } else {
            Ok(Some(result.tokens.context("no outputs")?.unpack()?))
        }
    }

    pub fn get_info(&self, state: &ton_block::AccountStuff) -> Result<DePoolInfo> {
        let info = self
            .run_local(state, common::get_depool_info(), &[])?
            .unpack()?;
        Ok(info)
    }

    pub fn get_rounds(&self, state: &ton_block::AccountStuff) -> Result<RoundsMap> {
        let rounds = self
            .run_local(state, common::get_rounds(), &[])?
            .unpack_first()?;
        Ok(rounds)
    }

    pub fn get_allowed_participants(
        &self,
        state: &ton_block::AccountStuff,
    ) -> Result<Vec<ton_block::MsgAddressInt>> {
        self.ensure_stever()?;
        let addresses: stever_v1::ParticipantsMap = self
            .run_local(state, stever_v1::allowed_participants(), &[])?
            .unpack_first()?;
        Ok(addresses.into_keys().collect())
    }

    fn run_local(
        &self,
        state: &ton_block::AccountStuff,
        function: &ton_abi::Function,
        inputs: &[ton_abi::Token],
    ) -> Result<Vec<ton_abi::Token>> {
        function
            .run_local(&SimpleClock, state.clone(), inputs)?
            .tokens
            .context("no outputs")
    }

    pub async fn get_state(&self) -> Result<ton_block::AccountStuff> {
        self.subscription
            .get_account_state(&self.address)
            .await?
            .context("DePool not deployed")
    }

    fn ensure_stever(&self) -> Result<()> {
        match self.ty {
            DePoolType::DefaultV3 => anyhow::bail!("expected StEver depool"),
            DePoolType::StEver => Ok(()),
        }
    }

    fn external_message_to_self<T>(&self, body: T) -> ton_block::Message
    where
        T: Into<ton_types::SliceData>,
    {
        let mut message =
            ton_block::Message::with_ext_in_header(ton_block::ExternalInboundMessageHeader {
                dst: self.address.clone(),
                ..Default::default()
            });
        message.set_body(body.into());
        message
    }

    fn internal_message_to_self<T>(&self, amount: u128, payload: T) -> InternalMessage
    where
        T: Into<ton_types::Cell>,
    {
        InternalMessage {
            amount,
            dst: self.address.clone(),
            payload: payload.into(),
        }
    }
}

impl DePoolType {
    pub fn compute_depool_address(
        &self,
        pubkey: &ed25519_dalek::PublicKey,
    ) -> Result<ton_block::MsgAddressInt> {
        let state_init = self
            .compute_depool_state_init(pubkey)
            .context("failed to compute state init")?
            .serialize()
            .context("failed to serialize DePool state")?;
        ton_block::MsgAddressInt::with_standart(None, 0, state_init.repr_hash().into())
            .context("failed to compute DePool address")
    }

    fn compute_depool_state_init(
        &self,
        pubkey: &ed25519_dalek::PublicKey,
    ) -> Result<ton_block::StateInit> {
        let mut state_init = self.depool_tvc().clone();
        if let Some(data) = state_init.data.take() {
            let data = ton_abi::Contract::insert_pubkey(data.into(), pubkey.as_bytes())
                .context("failed to insert pubkey")?;
            state_init.data = Some(data.into_cell());
        }
        Ok(state_init)
    }
}

macro_rules! impl_getters(
    ($t:ident, $tvc:ident, $proxy:ident, { $($var:ident => ($tvc_path:literal, $proxy_path:literal)),*$(,)? }) => {
        impl $t {
            pub fn $tvc(&self) -> &'static ton_block::StateInit {
                match self {
                    $(Self::$var => once!(ton_block::StateInit, || {
                        let bytes = include_bytes!($tvc_path);
                        ton_block::StateInit::construct_from_bytes(bytes).unwrap()
                    })),*,
                }
            }

            pub fn $proxy(&self) -> &'static ton_types::Cell {
                match self {
                    $(Self::$var => once!(ton_types::Cell, || {
                        let mut code = include_bytes!($proxy_path).as_ref();
                        ton_types::deserialize_tree_of_cells(&mut code).unwrap()
                    })),*,
                }
            }
        }
    };
);

impl_getters!(DePoolType, depool_tvc, proxy_code, {
    DefaultV3 => ("./v3/DePool.tvc", "./v3/DePoolProxy.code"),
    StEver => ("./stever/DePool.tvc", "./stever/DePoolProxy.code"),
});

#[derive(Clone, PackAbiPlain, KnownParamTypePlain)]
struct ConstructorInputs {
    #[abi(uint64)]
    min_stake: u64,
    #[abi(uint64)]
    validator_assurance: u64,
    #[abi(cell)]
    proxy_code: ton_types::Cell,
    #[abi(address)]
    validator_wallet: ton_block::MsgAddressInt,
    #[abi(uint8)]
    participant_reward_fraction: u8,
}

#[derive(Debug, Clone, UnpackAbiPlain, KnownParamTypePlain)]
pub struct ParticipantInfo {
    #[abi(uint64)]
    pub total: u64,
    #[abi(uint64)]
    pub withdraw_value: u64,
    #[abi(bool)]
    pub reinvest: bool,
    #[abi(uint64)]
    pub reward: u64,
    #[abi]
    pub stakes: BTreeMap<u64, u64>,
    #[abi]
    pub vestings: BTreeMap<u64, ComplexStake>,
    #[abi]
    pub locks: BTreeMap<u64, ComplexStake>,
    #[abi(address)]
    pub vesting_donor: ton_block::MsgAddressInt,
    #[abi(address)]
    pub lock_donor: ton_block::MsgAddressInt,
}

impl ParticipantInfo {
    pub fn compute_total_stake(&self, round: u64) -> u64 {
        let mut result = 0;
        if let Some(ordinary) = self.stakes.get(&round) {
            result += ordinary;
        }
        if let Some(vesting) = self.vestings.get(&round) {
            result += vesting.remaining_amount;
        }
        if let Some(lock) = self.locks.get(&round) {
            result += lock.remaining_amount;
        }
        result
    }
}

#[derive(Debug, Clone, UnpackAbi, KnownParamType)]
pub struct ComplexStake {
    #[abi(uint64)]
    pub remaining_amount: u64,
    #[abi(uint64)]
    pub last_withdrawal_time: u64,
    #[abi(uint32)]
    pub withdrawal_period: u32,
    #[abi(uint64)]
    pub withdrawal_value: u64,
    #[abi(address)]
    pub owner: ton_block::MsgAddressInt,
}

#[derive(Clone, UnpackAbiPlain, KnownParamTypePlain)]
pub struct DePoolInfo {
    #[abi(bool)]
    pub pool_closed: bool,
    #[abi(uint64)]
    pub min_stake: u64,
    #[abi(uint64)]
    pub validator_assurance: u64,
    #[abi(uint8)]
    pub participant_reward_fraction: u8,
    #[abi(uint8)]
    pub validator_reward_fraction: u8,
    #[abi(uint64)]
    pub balance_threshold: u64,
    #[abi(address)]
    pub validator_wallet: ton_block::MsgAddressInt,
    #[abi(array)]
    pub proxies: Vec<ton_block::MsgAddressInt>,
    #[abi(uint64)]
    pub stake_fee: u64,
    #[abi(uint64)]
    pub return_or_reinvest_fee: u64,
    #[abi(uint64)]
    pub proxy_fee: u64,
}

#[derive(Debug, UnpackAbi, KnownParamType)]
pub struct Round {
    #[abi(uint64)]
    pub id: u64,
    #[abi(uint32)]
    pub supposed_elected_at: u32,
    #[abi(uint32)]
    pub unfreeze: u32,
    #[abi(uint32)]
    pub stake_held_for: u32,
    #[abi(uint256)]
    pub vset_hash_in_election_phase: ton_types::UInt256,
    #[abi]
    pub step: RoundStep,
    #[abi]
    pub completion_reason: CompletionReason,
    #[abi(uint64)]
    pub stake: u64,
    #[abi(uint64)]
    pub recovered_stake: u64,
    #[abi(uint64)]
    pub unused: u64,
    #[abi(bool)]
    pub is_validator_stake_completed: bool,
    #[abi(uint64)]
    pub participant_reward: u64,
    #[abi(uint32)]
    pub participant_qty: u32,
    #[abi(uint64)]
    pub validator_stake: u64,
    #[abi(uint64)]
    pub validator_remaining_stake: u64,
    #[abi(uint64)]
    pub handled_stakes_and_rewards: u64,
}

pub type RoundsMap = BTreeMap<u64, Round>;

#[derive(Copy, Clone, Debug, PartialEq, Eq, UnpackAbi, KnownParamType)]
pub enum RoundStep {
    /// Receiving a half of vesting/lock stake from participants
    PrePooling = 0,
    /// Receiving stakes from participants
    Pooling = 1,
    /// Waiting for the election request from the validator
    WaitingValidatorRequest = 2,
    /// Stake has been sent to the elector. Waiting for the answer from the elector
    WaitingIfStakeAccepted = 3,
    /// Elector has accepted round stake. Validator is a candidate.
    /// Waiting validation start to know if we won the elections
    WaitingValidationStart = 4,
    /// DePool has tried to recover stake during validation period to know
    /// if we won the elections.  Waiting for elector answer
    WaitingIfValidatorWinElections = 5,
    /// If CompletionReason!=Undefined, then the round is completed and we are waiting for
    /// return/reinvest funds after the next round. Otherwise if validator won the elections,
    /// waiting for the end of unfreeze period
    WaitingUnfreeze = 6,
    /// Unfreeze period has been ended.
    /// Requested recovering stake from the elector. Waiting for the answer
    WaitingReward = 7,
    /// Returning or reinvesting participant stakes because round is completed
    Completing = 8,
    /// All round states are returned or reinvested
    Completed = 9,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, UnpackAbi, KnownParamType)]
pub enum CompletionReason {
    Undefined = 0,
    PoolClosed = 1,
    FakeRound = 2,
    ValidatorStakeIsTooSmall = 3,
    StakeIsRejectedByElector = 4,
    RewardIsReceived = 5,
    ElectionsAreLost = 6,
    ValidatorIsPunished = 7,
    NoValidatorRequest = 8,
}

mod common {
    use super::*;

    pub fn constructor() -> &'static ton_abi::Function {
        once!(ton_abi::Function, || {
            FunctionBuilder::new("constructor")
                .time_header()
                .expire_header()
                .inputs(ConstructorInputs::param_type())
                .build()
        })
    }

    pub fn add_ordinary_stake() -> &'static ton_abi::Function {
        once!(ton_abi::Function, || {
            FunctionBuilder::new("addOrdinaryStake")
                .input("stake", u64::param_type())
                .build()
        })
    }

    pub fn ticktock() -> &'static ton_abi::Function {
        once!(ton_abi::Function, || {
            FunctionBuilder::new("ticktock").build()
        })
    }

    pub fn receive_funds() -> &'static ton_abi::Function {
        once!(ton_abi::Function, || {
            FunctionBuilder::new("receiveFunds").build()
        })
    }

    pub fn get_depool_balance() -> &'static ton_abi::Function {
        once!(ton_abi::Function, || {
            FunctionBuilder::new("getDePoolBalance")
                .time_header()
                .expire_header()
                .abi_version(ton_abi::contract::ABI_VERSION_2_0)
                .output("balance", ton_abi::ParamType::Int(256))
                .build()
        })
    }

    pub fn get_participant_info() -> &'static ton_abi::Function {
        once!(ton_abi::Function, || {
            FunctionBuilder::new("getParticipantInfo")
                .time_header()
                .expire_header()
                .abi_version(ton_abi::contract::ABI_VERSION_2_0)
                .input("addr", ton_block::MsgAddressInt::param_type())
                .outputs(ParticipantInfo::param_type())
                .build()
        })
    }

    pub fn get_depool_info() -> &'static ton_abi::Function {
        once!(ton_abi::Function, || {
            FunctionBuilder::new("getDePoolInfo")
                .time_header()
                .expire_header()
                .outputs(DePoolInfo::param_type())
                .build()
        })
    }

    pub fn get_rounds() -> &'static ton_abi::Function {
        once!(ton_abi::Function, || {
            FunctionBuilder::new("getRounds")
                .output("rounds", RoundsMap::param_type())
                .time_header()
                .expire_header()
                .build()
        })
    }
}

mod stever_v1 {
    use super::*;

    pub fn get_participant_info() -> &'static ton_abi::Function {
        once!(ton_abi::Function, || {
            FunctionBuilder::new("getParticipantInfo")
                .abi_version(ton_abi::contract::ABI_VERSION_2_2)
                .time_header()
                .expire_header()
                .input("addr", ton_block::MsgAddressInt::param_type())
                .outputs(ParticipantInfo::param_type())
                .build()
        })
    }

    pub fn set_allowed_participant() -> &'static ton_abi::Function {
        once!(ton_abi::Function, || {
            FunctionBuilder::new("setAllowedParticipant")
                .abi_version(ABI_VERSION_2_2)
                .input("addr", ton_block::MsgAddressInt::param_type())
                .build()
        })
    }

    pub type ParticipantsMap = BTreeMap<ton_block::MsgAddressInt, bool>;

    pub fn allowed_participants() -> &'static ton_abi::Function {
        once!(ton_abi::Function, || {
            FunctionBuilder::new("allowedParticipants")
                .abi_version(ABI_VERSION_2_2)
                .output("participants", ParticipantsMap::param_type())
                .time_header()
                .expire_header()
                .build()
        })
    }
}
