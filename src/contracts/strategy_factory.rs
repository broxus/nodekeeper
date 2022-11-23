use std::sync::Arc;

use anyhow::{Context, Result};
use nekoton_abi::{
    BuildTokenValue, EventBuilder, FunctionBuilder, KnownParamType, KnownParamTypePlain,
    TokenValueExt, UnpackAbi, UnpackAbiPlain, UnpackFirst,
};

use super::{InternalMessage, ONE_EVER};
use crate::network::Subscription;
use crate::util::TransactionWithHash;

pub struct StrategyFactory {
    address: ton_block::MsgAddressInt,
    subscription: Arc<Subscription>,
}

impl StrategyFactory {
    pub const DEPLOYMENT_FEE: u128 = 22 * ONE_EVER;

    pub fn new(address: ton_block::MsgAddressInt, subscription: Arc<Subscription>) -> Self {
        Self {
            address,
            subscription,
        }
    }

    pub async fn get_details(&self) -> Result<Details> {
        let details = self
            .subscription
            .run_local(
                &self.address,
                methods::get_details(),
                &[0u32.token_value().named("answerId")],
            )
            .await?
            .unpack_first()?;
        Ok(details)
    }

    pub fn deploy_strategy(&self, depool: &ton_block::MsgAddressInt) -> Result<InternalMessage> {
        Ok(InternalMessage {
            amount: Self::DEPLOYMENT_FEE,
            dst: self.address.clone(),
            payload: methods::deploy_strategy()
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
                let event: NewStrategyDeployed = ton_abi::TokenValue::decode_params(
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
}

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

mod methods {
    use super::*;

    pub fn get_details() -> &'static ton_abi::Function {
        once!(ton_abi::Function, || {
            FunctionBuilder::new("getDetails")
                .abi_version(ABI_VERSION)
                .pubkey_header()
                .time_header()
                .expire_header()
                .input("answerId", u32::param_type())
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

#[derive(Clone, UnpackAbiPlain, KnownParamTypePlain)]
struct NewStrategyDeployed {
    #[abi(address)]
    strategy: ton_block::MsgAddressInt,
    #[abi(address)]
    _depool: ton_block::MsgAddressInt,
    #[abi(uint32)]
    _version: u32,
}

mod events {
    use super::*;

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
