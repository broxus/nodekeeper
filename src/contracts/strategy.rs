use std::sync::Arc;

use anyhow::Result;
use nekoton_abi::{
    BuildTokenValue, FunctionBuilder, KnownParamType, TokenValueExt, UnpackAbi, UnpackFirst,
};

use crate::subscription::Subscription;

pub struct Strategy {
    pub address: ton_block::MsgAddressInt,
    pub subscription: Arc<Subscription>,
}

impl Strategy {
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
}

#[derive(Clone, UnpackAbi, KnownParamType)]
pub struct Details {
    #[abi(address)]
    pub vault: ton_block::MsgAddressInt,
    #[abi(address)]
    pub depool: ton_block::MsgAddressInt,
    #[abi(uint32)]
    pub strategy_version: u32,
    #[abi(uint8)]
    pub state: u8,
}

mod methods {
    use super::*;

    pub fn get_details() -> &'static ton_abi::Function {
        once!(ton_abi::Function, || {
            FunctionBuilder::new("getDetails")
                .abi_version(ABI_VERSION)
                .time_header()
                .expire_header()
                .input("answerId", u32::param_type())
                .output("details", Details::param_type())
                .build()
        })
    }
}

const ABI_VERSION: ton_abi::contract::AbiVersion = ton_abi::contract::ABI_VERSION_2_2;
