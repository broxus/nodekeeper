use std::sync::Arc;

use anyhow::{Context, Result};
use nekoton_abi::{FunctionBuilder, KnownParamTypePlain, PackAbiPlain};
use ton_abi::contract::ABI_VERSION_2_3;
use ton_block::GetRepresentationHash;

use super::{InternalMessage, ONE_EVER};
use crate::subscription::Subscription;
use crate::util::make_default_headers;

pub struct Wallet {
    keypair: ed25519_dalek::Keypair,
    address: ton_block::MsgAddressInt,
    subscription: Arc<Subscription>,
}

impl Wallet {
    pub fn new(
        workchain_id: i8,
        keypair: ed25519_dalek::Keypair,
        subscription: Arc<Subscription>,
    ) -> Result<Self> {
        let address = compute_wallet_address(workchain_id, &keypair.public)
            .context("failed to compute wallet address")?;
        Ok(Self {
            keypair,
            address,
            subscription,
        })
    }

    pub async fn get_balance(&self) -> Result<Option<u128>> {
        let account = self.get_account_state().await?;
        Ok(account.map(|state| state.storage.balance.grams.0))
    }

    pub async fn transfer(&self, internal_message: InternalMessage) -> Result<()> {
        let account = self.get_account_state().await?;

        let state_init = match account {
            Some(account) => match account.storage.state {
                ton_block::AccountState::AccountActive { .. } => None,
                ton_block::AccountState::AccountFrozen { .. } => {
                    anyhow::bail!("account frozen");
                }
                ton_block::AccountState::AccountUninit => Some(
                    make_state_init(&self.keypair.public).context("failed to make state init")?,
                ),
            },
            None => anyhow::bail!("account not deployed"),
        };

        let inputs = ever_wallet::SendTransactionInputs {
            dest: internal_message.dst,
            value: internal_message.amount.into(),
            bounce: false,
            flags: 3,
            payload: internal_message.payload,
        }
        .pack();

        self.subscription
            .send_message_with_retires(|timeout| {
                let (expire_at, headers) = make_default_headers(Some(self.keypair.public), timeout);

                let mut message = ton_block::Message::with_ext_in_header(
                    ton_block::ExternalInboundMessageHeader {
                        dst: self.address.clone(),
                        ..Default::default()
                    },
                );

                message.set_body(
                    ever_wallet::send_transaction()
                        .encode_input(
                            &headers,
                            &inputs,
                            false,
                            Some(&self.keypair),
                            Some(self.address.clone()),
                        )?
                        .into(),
                );

                if let Some(state_init) = state_init.clone() {
                    message.set_state_init(state_init);
                }

                Ok((message, expire_at))
            })
            .await?;
        Ok(())
    }

    async fn get_account_state(&self) -> Result<Option<ton_block::AccountStuff>> {
        self.subscription
            .get_account_state(&self.address)
            .await
            .context("failed to get account state")
    }
}

pub struct Gift {
    pub amount: u64,
    pub dest: ton_block::MsgAddressInt,
    pub payload: ton_types::Cell,
}

const BALANCE_OFFSET: u64 = ONE_EVER / 5;

pub fn compute_wallet_address(
    workchain_id: i8,
    pubkey: &ed25519_dalek::PublicKey,
) -> Result<ton_block::MsgAddressInt> {
    let hash = make_state_init(pubkey).and_then(|state| state.hash())?;
    Ok(ton_block::MsgAddressInt::AddrStd(
        ton_block::MsgAddrStd::with_address(None, workchain_id, hash.into()),
    ))
}

fn make_state_init(public_key: &ed25519_dalek::PublicKey) -> Result<ton_block::StateInit> {
    use ton_types::IBitstring;

    let mut data = ton_types::BuilderData::new();
    data.append_raw(public_key.as_bytes(), 256)?.append_u64(0)?;
    let data = data.into_cell()?;

    Ok(ton_block::StateInit {
        code: Some(ever_wallet_code().clone()),
        data: Some(data),
        ..Default::default()
    })
}

fn ever_wallet_code() -> &'static ton_types::Cell {
    once!(ton_types::Cell, || {
        let mut data = include_bytes!("./EverWallet.code").as_ref();
        ton_types::deserialize_tree_of_cells(&mut data).unwrap()
    })
}

mod ever_wallet {
    use super::*;

    #[derive(Clone, PackAbiPlain, KnownParamTypePlain)]
    pub struct SendTransactionInputs {
        #[abi(address)]
        pub dest: ton_block::MsgAddressInt,
        #[abi(uint128)]
        pub value: u128,
        #[abi(bool)]
        pub bounce: bool,
        #[abi(uint8)]
        pub flags: u8,
        #[abi(cell)]
        pub payload: ton_types::Cell,
    }

    pub fn send_transaction() -> &'static ton_abi::Function {
        once!(ton_abi::Function, || {
            FunctionBuilder::new("sendTransaction")
                .abi_version(ABI_VERSION_2_3)
                .time_header()
                .expire_header()
                .pubkey_header()
                .inputs(SendTransactionInputs::param_type())
                .build()
        })
    }
}
