use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use futures_util::stream::{FuturesUnordered, StreamExt};
use nekoton_abi::{FunctionBuilder, KnownParamType, UnpackFirst};

use super::strategy::Strategy;
use crate::network::Subscription;

pub struct Cluster {
    pub address: ton_block::MsgAddressInt,
    pub subscription: Arc<Subscription>,
}

impl Cluster {
    pub fn new(address: ton_block::MsgAddressInt, subscription: Arc<Subscription>) -> Self {
        Self {
            address,
            subscription,
        }
    }

    pub async fn find_deployed_strategy_for_depool(
        &self,
        depool: &ton_block::MsgAddressInt,
    ) -> Result<Option<ton_block::MsgAddressInt>> {
        const CHUNK_LEN: usize = 10;

        let all_strategies = self
            .get_deployed_strategies()
            .await
            .context("failed to get all deployed strategies")?;

        for chunk in all_strategies.chunks(CHUNK_LEN) {
            let mut futures = FuturesUnordered::new();
            for strategy in chunk {
                let strategy = Strategy::new(strategy.clone(), self.subscription.clone());
                futures.push(async move {
                    let details = strategy.get_details().await;
                    (strategy.address, details)
                });
            }

            while let Some((strategy, details)) = futures.next().await {
                match details {
                    Ok(details) => {
                        if &details.depool == depool {
                            return Ok(Some(strategy));
                        }
                    }
                    Err(e) => {
                        tracing::warn!(%strategy, "failed to get strategy details: {e:?}");
                    }
                }
            }
        }

        Ok(None)
    }

    pub async fn get_deployed_strategies(&self) -> Result<Vec<ton_block::MsgAddressInt>> {
        let details: StrategiesMap = self
            .subscription
            .run_local(&self.address, methods::deployed_strategies(), &[])
            .await?
            .unpack_first()?;

        Ok(details.into_keys().collect())
    }
}

type StrategiesMap = BTreeMap<ton_block::MsgAddressInt, bool>;

mod methods {
    use super::*;

    pub fn deployed_strategies() -> &'static ton_abi::Function {
        once!(ton_abi::Function, || {
            FunctionBuilder::new("deployedStrategies")
                .abi_version(ABI_VERSION)
                .default_headers()
                .output("details", StrategiesMap::param_type())
                .build()
        })
    }
}

const ABI_VERSION: ton_abi::contract::AbiVersion = ton_abi::contract::ABI_VERSION_2_2;
