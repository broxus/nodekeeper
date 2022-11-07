use std::sync::Arc;

use anyhow::{Context, Result};
use nekoton_abi::{
    BuildTokenValue, EventBuilder, FunctionBuilder, FunctionExt, KnownParamType,
    KnownParamTypePlain, TokenValueExt, UnpackAbi, UnpackAbiPlain, UnpackFirst,
};
use nekoton_utils::SimpleClock;

use crate::subscription::Subscription;
use crate::util::TransactionWithHash;

use super::{InternalMessage, ONE_EVER};

pub struct StrategyFactory {
    pub address: ton_block::MsgAddressInt,
    pub subscription: Arc<Subscription>,
}

impl StrategyFactory {
    pub const DEPLOYMENT_FEE: u128 = 23 * ONE_EVER;

    pub fn new(address: ton_block::MsgAddressInt, subscription: Arc<Subscription>) -> Self {
        Self {
            address,
            subscription,
        }
    }

    pub async fn get_details(&self) -> Result<factory::Details> {
        let details = self
            .run_local(factory::get_details(), &[])
            .await?
            .unpack_first()?;
        Ok(details)
    }

    pub fn deploy_strategy(&self, depool: &ton_block::MsgAddressInt) -> Result<InternalMessage> {
        Ok(InternalMessage {
            amount: Self::DEPLOYMENT_FEE,
            dst: self.address.clone(),
            payload: factory::deploy_strategy()
                .encode_internal_input(&[depool.token_value().named("depool")])?
                .into(),
        })
    }

    pub fn extract_strategy_address(tx: TransactionWithHash) -> Result<ton_block::MsgAddressInt> {
        let mut address = None;
        tx.data.out_msgs.iterate(|ton_block::InRefValue(msg)| {
            if !matches!(msg.header(), ton_block::CommonMsgInfo::ExtOutMsgInfo(..)) {
                return Ok(true);
            }

            let Some(mut body) = msg.body() else { return Ok(true); };
            let Ok(function_id) = body.get_next_u32() else { return Ok(true); };

            if function_id == events::new_strategy_deployed().id {
                let event: events::NewStrategyDeployed = ton_abi::TokenValue::decode_params(
                    &events::new_strategy_deployed().inputs,
                    body,
                    &ABI_VERSION,
                    false,
                )?
                .unpack()?;

                address = Some(event.strategy);
                Ok(false)
            } else {
                Ok(true)
            }
        })?;
        address.context("strategy deployment event not found")
    }

    async fn run_local(
        &self,
        function: &ton_abi::Function,
        inputs: &[ton_abi::Token],
    ) -> Result<Vec<ton_abi::Token>> {
        let account = self
            .subscription
            .get_account_state(&self.address)
            .await?
            .context("factory not deployed")?;
        function
            .run_local(&SimpleClock, account, inputs)?
            .tokens
            .context("no output")
    }
}

mod factory {
    use super::*;

    #[derive(Clone, UnpackAbi, KnownParamType)]
    pub struct Details {
        #[abi(address)]
        pub stever_vault: ton_block::MsgAddressInt,
        #[abi(address)]
        pub owner: ton_block::MsgAddressInt,
        #[abi(uint32)]
        pub strategy_version: u32,
        #[abi(uint32)]
        pub strategy_count: u32,
        #[abi(uint32)]
        pub factory_version: u32,
    }

    pub fn get_details() -> &'static ton_abi::Function {
        once!(ton_abi::Function, || {
            FunctionBuilder::new("getDetails")
                .abi_version(ABI_VERSION)
                .pubkey_header()
                .time_header()
                .expire_header()
                .output("details", Details::param_type())
                .build()
        })
    }

    pub fn deploy_strategy() -> &'static ton_abi::Function {
        once!(ton_abi::Function, || {
            FunctionBuilder::new("deployStrategy")
                .abi_version(ABI_VERSION)
                .input("depool", ton_block::MsgAddressInt::param_type())
                .build()
        })
    }
}

mod events {
    use super::*;

    #[derive(Clone, UnpackAbiPlain, KnownParamTypePlain)]
    pub struct NewStrategyDeployed {
        #[abi(address)]
        pub strategy: ton_block::MsgAddressInt,
        #[abi(address)]
        pub depool: ton_block::MsgAddressInt,
        #[abi(uint32)]
        pub version: u32,
    }

    pub fn new_strategy_deployed() -> &'static ton_abi::Event {
        once!(ton_abi::Event, || {
            EventBuilder::new("NewStrategyDeployed")
                .abi_version(ABI_VERSION)
                .inputs(NewStrategyDeployed::param_type())
                .build()
        })
    }
}

const ABI_VERSION: ton_abi::contract::AbiVersion = ton_abi::contract::ABI_VERSION_2_2;
