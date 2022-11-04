use std::collections::BTreeMap;
use std::sync::Arc;

use anyhow::{Context, Result};
use nekoton_abi::{
    BuildTokenValue, FunctionBuilder, FunctionExt, KnownParamType, KnownParamTypePlain,
    PackAbiPlain, TokenValueExt, UnpackAbi, UnpackAbiPlain, UnpackFirst,
};
use nekoton_utils::SimpleClock;
use ton_abi::contract::ABI_VERSION_2_2;
use ton_block::{Deserializable, Serializable};

use super::{InternalMessage, ONE_EVER};
use crate::config::DePoolType;
use crate::subscription::Subscription;
use crate::util::make_default_headers;

pub struct DePool {
    ty: DePoolType,
    keypair: ed25519_dalek::Keypair,
    address: ton_block::MsgAddressInt,
    subscription: Arc<Subscription>,
}

impl DePool {
    pub fn new(
        ty: DePoolType,
        keypair: ed25519_dalek::Keypair,
        subscription: Arc<Subscription>,
    ) -> Result<Self> {
        let address = ty.compute_depool_address(&keypair.public)?;

        Ok(Self {
            ty,
            keypair,
            address,
            subscription,
        })
    }

    pub async fn deploy(&self, inputs: common::ConstructorInputs) -> Result<()> {
        let inputs = inputs.pack();

        self.subscription
            .send_message_with_retires(move |timeout| {
                let (expire_at, header) = make_default_headers(None, timeout);

                let mut message = self.external_message_to_self(
                    common::constructor()
                        .encode_input(
                            &header,
                            &inputs,
                            false,
                            Some(&self.keypair),
                            Some(self.address.clone()),
                        )
                        .context("failed to encode constructor")?,
                );

                message.set_state_init(self.ty.compute_depool_state_init(&self.keypair.public)?);

                Ok((message, expire_at))
            })
            .await
            .context("failed to send constructor message")?;

        Ok(())
    }

    pub async fn terminate(&self) -> Result<()> {
        self.subscription
            .send_message_with_retires(move |timeout| {
                let (expire_at, header) = make_default_headers(None, timeout);

                let message = self.external_message_to_self(
                    common::terminator()
                        .encode_input(
                            &header,
                            &[],
                            false,
                            Some(&self.keypair),
                            Some(self.address.clone()),
                        )
                        .context("failed to encode termination message")?,
                );

                Ok((message, expire_at))
            })
            .await
            .context("failed to send termination message")?;

        Ok(())
    }

    pub fn ticktock(&self) -> Result<InternalMessage> {
        Ok(self.internal_message_to_self(ONE_EVER, common::ticktock().encode_internal_input(&[])?))
    }

    pub fn add_ordinary_stake(&self, amount: u64) -> Result<InternalMessage> {
        Ok(self.internal_message_to_self(
            amount + ONE_EVER / 2,
            &common::add_ordinary_stake()
                .encode_internal_input(&[amount.token_value().named("stake")])?,
        ))
    }

    pub fn participate_in_elections(
        &self,
        inputs: common::ParticipateInElectionsInputs,
    ) -> Result<InternalMessage> {
        Ok(self.internal_message_to_self(
            ONE_EVER,
            common::participate_in_elections().encode_internal_input(&inputs.pack())?,
        ))
    }

    pub fn set_allowed_participant(
        &self,
        address: ton_block::MsgAddressInt,
    ) -> Result<InternalMessage> {
        self.ensure_stever()?;
        Ok(self.internal_message_to_self(
            ONE_EVER,
            stever_v1::set_allowed_participant()
                .encode_internal_input(&[address.token_value().named("addr")])?,
        ))
    }

    pub async fn get_info(&self) -> Result<common::DePoolInfo> {
        let info = self
            .run_local(common::get_depool_info(), &[])
            .await?
            .unpack()?;
        Ok(info)
    }

    pub async fn get_rounds(&self) -> Result<common::RoundsMap> {
        let rounds = self
            .run_local(common::get_rounds(), &[])
            .await?
            .unpack_first()?;
        Ok(rounds)
    }

    pub async fn get_allowed_participants(&self) -> Result<Vec<ton_block::MsgAddressInt>> {
        self.ensure_stever()?;
        let addresses: stever_v1::ParticipantsMap = self
            .run_local(stever_v1::allowed_participants(), &[])
            .await?
            .unpack_first()?;
        Ok(addresses.into_keys().collect())
    }

    async fn run_local(
        &self,
        function: &ton_abi::Function,
        inputs: &[ton_abi::Token],
    ) -> Result<Vec<ton_abi::Token>> {
        let account = self.get_state().await?;
        function
            .run_local(&SimpleClock, account, inputs)?
            .tokens
            .context("no outputs")
    }

    async fn get_state(&self) -> Result<ton_block::AccountStuff> {
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

    fn internal_message_to_self<T>(&self, amount: u64, payload: T) -> InternalMessage
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
    fn compute_depool_address(
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

mod common {
    use super::*;

    #[derive(Clone, PackAbiPlain, KnownParamTypePlain)]
    pub struct ConstructorInputs {
        #[abi(uint64)]
        pub min_stake: u64,
        #[abi(uint64)]
        pub validator_assurance: u64,
        #[abi(cell)]
        pub proxy_code: ton_types::Cell,
        #[abi(address)]
        pub validator_wallet: ton_block::MsgAddressInt,
        #[abi(uint8)]
        pub participant_reward_fraction: u8,
    }

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

    #[derive(Clone, PackAbiPlain, KnownParamTypePlain)]
    pub struct ParticipateInElectionsInputs {
        #[abi(uint64)]
        query_id: u64,
        #[abi(uint256)]
        validator_key: ton_types::UInt256,
        #[abi(uint32)]
        stake_at: u32,
        #[abi(uint32)]
        max_factor: u32,
        #[abi(uint256)]
        adnl_addr: ton_types::UInt256,
        #[abi(bytes)]
        signature: Vec<u8>,
    }

    pub fn participate_in_elections() -> &'static ton_abi::Function {
        once!(ton_abi::Function, || {
            FunctionBuilder::new("participateInElections")
                .id(0x4E73744B)
                .inputs(ParticipateInElectionsInputs::param_type())
                .build()
        })
    }

    pub fn ticktock() -> &'static ton_abi::Function {
        once!(ton_abi::Function, || {
            FunctionBuilder::new("ticktock").build()
        })
    }

    pub fn terminator() -> &'static ton_abi::Function {
        once!(ton_abi::Function, || {
            FunctionBuilder::new("terminator").build()
        })
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

    pub fn get_depool_info() -> &'static ton_abi::Function {
        once!(ton_abi::Function, || {
            FunctionBuilder::new("getDePoolInfo")
                .time_header()
                .expire_header()
                .outputs(DePoolInfo::param_type())
                .build()
        })
    }

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

    #[derive(UnpackAbi, KnownParamType)]
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
