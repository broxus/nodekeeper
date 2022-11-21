use std::collections::BTreeMap;
use std::sync::Arc;

use anyhow::{Context, Result};
use broxus_util::now;
use nekoton_abi::{
    BuildTokenValue, FunctionBuilder, KnownParamType, KnownParamTypePlain, MaybeRef, PackAbiPlain,
    TokenValueExt, UnpackAbi, UnpackAbiPlain,
};

use super::{InternalMessage, ONE_EVER};
use crate::subscription::Subscription;
use crate::util::split_address;

pub struct Elector {
    address: ton_block::MsgAddressInt,
    subscription: Arc<Subscription>,
}

impl Elector {
    pub fn new(address: ton_types::UInt256, subscription: Arc<Subscription>) -> Self {
        let address = ton_block::MsgAddressInt::AddrStd(ton_block::MsgAddrStd {
            anycast: None,
            workchain_id: -1,
            address: address.into(),
        });

        Self {
            address,
            subscription,
        }
    }

    pub fn address(&self) -> &ton_block::MsgAddressInt {
        &self.address
    }

    pub fn recover_stake(&self) -> Result<InternalMessage> {
        let now = now() as u64;
        Ok(InternalMessage {
            amount: ONE_EVER,
            dst: self.address.clone(),
            payload: methods::recover_stake()
                .encode_internal_input(&[now.token_value().named("query_id")])
                .and_then(ton_types::BuilderData::into_cell)?,
        })
    }

    /// Prepares validator node and generates elector payload
    pub async fn participate_in_elections(
        &self,
        election_id: u32,
        address: &ton_block::MsgAddressInt,
        stake_factor: u32,
        timings: &ton_block::ConfigParam15,
    ) -> Result<ton_types::Cell> {
        const TTL_OFFSET: u32 = 1000;

        anyhow::ensure!(
            address.is_masterchain(),
            "participant address not in masterchain"
        );

        let (_, address) = split_address(address)?;

        let rpc = self.subscription.tcp_rpc();

        // Generate new key
        let permanent_key_hash = rpc
            .generate_key_pair()
            .await
            .context("failed to generate validator keys")?;

        // Export its public key
        let perm_pubkey = rpc
            .export_public_key(&permanent_key_hash)
            .await
            .context("failed to export validator public key")?;

        // Add this key as a validator key
        let ttl = election_id
            + timings.validators_elected_for
            + timings.elections_start_before
            + timings.elections_end_before
            + timings.stake_held_for
            + TTL_OFFSET;
        rpc.add_validator_permanent_key(&permanent_key_hash, election_id, ttl)
            .await
            .context("failed to add validator permanent key")?;

        // Generate adnl key (key hash is equal to adnl addr)
        let adnl_addr = rpc
            .generate_key_pair()
            .await
            .context("failed to generate validator adnl keys")?;

        // Assign adnl address to the validator key
        // NOTE: ttl is 0 here because it is unused in the node
        rpc.add_validator_adnl_address(&permanent_key_hash, &adnl_addr, 0)
            .await
            .context("failed to add validator adnl address")?;

        // Sign data
        let unsigned = UnsignedParticipantData {
            election_id,
            address,
            max_factor: stake_factor,
            public_key: ton_types::UInt256::from(perm_pubkey.to_bytes()),
            adnl_addr: ton_types::UInt256::from(adnl_addr),
        };

        let data_to_sign = unsigned.build_data_to_sign();
        let signature = rpc
            .sign(&permanent_key_hash, &data_to_sign)
            .await
            .context("failed to sign election data")?;

        // Generate internal message payload
        unsigned
            .sign(signature)
            .context("failed to insert signature")
    }

    pub async fn get_data(&self) -> Result<ElectorData> {
        let state = self.get_state().await?;

        let ton_block::AccountState::AccountActive { state_init } = state.storage.state else {
            anyhow::bail!("elector account is not active");
        };

        let data = state_init.data.context("elector data is empty")?;
        let inner: data::PartialElectorData = ton_abi::TokenValue::decode_params(
            data::layout(),
            data.into(),
            &ton_abi::contract::ABI_VERSION_2_1,
            true,
        )
        .context("failed to parse elector data")?
        .unpack()?;

        Ok(ElectorData { inner })
    }

    async fn get_state(&self) -> Result<ton_block::AccountStuff> {
        self.subscription
            .get_account_state(&self.address)
            .await
            .context("failed to get elector state")?
            .context("elector not found")
    }
}

pub struct ElectorData {
    inner: data::PartialElectorData,
}

impl ElectorData {
    pub fn election_id(&self) -> Option<u32> {
        let election_id = self.inner.current_election.0.as_ref()?.elect_at;
        Some(election_id)
    }

    pub fn nearest_unfreeze_at(&self, election_id: u32) -> Option<u32> {
        self.inner
            .past_elections
            .values()
            .map(|election| election.unfreeze_at)
            .find(|&unfreeze_at| unfreeze_at < election_id)
    }

    pub fn has_unfrozen_stake(
        &self,
        address: &ton_block::MsgAddressInt,
    ) -> Option<ton_block::Grams> {
        if !address.is_masterchain() {
            // Elector has rewards only for masterchain accounts
            return None;
        }

        let (_, address) = split_address(address).ok()?;
        self.inner.credits.get(&address).copied()
    }

    pub fn elected(&self, address: &ton_block::MsgAddressInt) -> bool {
        if !address.is_masterchain() {
            return false;
        }

        let Some(current_election) = &self.inner.current_election.0 else { return false };
        let Ok((_, address)) = split_address(address) else { return false };

        current_election
            .members
            .values()
            .any(|entry| entry.src_addr == address)
    }
}

struct UnsignedParticipantData {
    election_id: u32,
    address: ton_types::UInt256,
    max_factor: u32,
    public_key: ton_types::UInt256,
    adnl_addr: ton_types::UInt256,
}

impl UnsignedParticipantData {
    fn build_data_to_sign(&self) -> Vec<u8> {
        const TL_ID: u32 = 0x654C5074;

        let mut data = Vec::with_capacity(4 + 4 + 4 + 32 + 32);
        data.extend_from_slice(&TL_ID.to_be_bytes());
        data.extend_from_slice(&self.election_id.to_be_bytes());
        data.extend_from_slice(&self.max_factor.to_be_bytes());
        data.extend_from_slice(self.address.as_array());
        data.extend_from_slice(self.adnl_addr.as_array());
        data
    }

    fn sign(self, signature: [u8; 64]) -> Result<ton_types::Cell> {
        methods::participate_in_elections()
            .encode_internal_input(
                &methods::ParticipateInElectionsInputs {
                    query_id: now() as u64,
                    validator_key: self.public_key,
                    stake_at: self.election_id,
                    max_factor: self.max_factor,
                    adnl_addr: self.adnl_addr,
                    signature: signature.to_vec(),
                }
                .pack(),
            )
            .and_then(ton_types::BuilderData::into_cell)
    }
}

mod data {
    use super::*;

    pub fn layout() -> &'static [ton_abi::Param] {
        once!(Vec<ton_abi::Param>, || PartialElectorData::param_type())
    }

    #[derive(Debug, UnpackAbiPlain, KnownParamTypePlain)]
    pub struct PartialElectorData {
        #[abi]
        pub current_election: MaybeRef<CurrentElectionData>,
        #[abi]
        pub credits: BTreeMap<ton_types::UInt256, ton_block::Grams>,
        #[abi]
        pub past_elections: BTreeMap<u32, PastElectionData>,
    }

    #[derive(Debug, UnpackAbi, KnownParamType)]
    pub struct CurrentElectionData {
        #[abi(uint32)]
        pub elect_at: u32,
        #[abi(uint32)]
        pub elect_close: u32,
        #[abi(gram)]
        pub min_stake: u128,
        #[abi(gram)]
        pub total_stake: u128,
        #[abi]
        pub members: BTreeMap<ton_types::UInt256, ElectionMember>,
        #[abi(bool)]
        pub failed: bool,
        #[abi(bool)]
        pub finished: bool,
    }

    #[derive(Debug, UnpackAbi, KnownParamType)]
    pub struct ElectionMember {
        #[abi(gram)]
        pub msg_value: u64,
        #[abi(uint32)]
        pub created_at: u32,
        #[abi(uint32)]
        pub max_factor: u32,
        #[abi(uint256)]
        pub src_addr: ton_types::UInt256,
        #[abi(uint256)]
        pub adnl_addr: ton_types::UInt256,
    }

    #[derive(Debug, UnpackAbi, KnownParamType)]
    pub struct PastElectionData {
        #[abi(uint32)]
        pub unfreeze_at: u32,
    }
}

mod methods {
    use super::*;

    pub fn recover_stake() -> &'static ton_abi::Function {
        once!(ton_abi::Function, || FunctionBuilder::new("recover_stake")
            .id(0x47657424)
            .input("query_id", u64::param_type())
            .build())
    }

    #[derive(Clone, PackAbiPlain, KnownParamTypePlain)]
    pub struct ParticipateInElectionsInputs {
        #[abi(uint64)]
        pub query_id: u64,
        #[abi(uint256)]
        pub validator_key: ton_types::UInt256,
        #[abi(uint32)]
        pub stake_at: u32,
        #[abi(uint32)]
        pub max_factor: u32,
        #[abi(uint256)]
        pub adnl_addr: ton_types::UInt256,
        #[abi(bytes)]
        pub signature: Vec<u8>,
    }

    pub fn participate_in_elections() -> &'static ton_abi::Function {
        once!(ton_abi::Function, || {
            FunctionBuilder::new("participate_in_elections")
                .id(0x4E73744B)
                .inputs(ParticipateInElectionsInputs::param_type())
                .build()
        })
    }
}
