use std::sync::Arc;

use anyhow::{Context, Result};
use nekoton_abi::{FunctionBuilder, KnownParamTypePlain, PackAbiPlain};
use ton_abi::contract::ABI_VERSION_2_3;
use ton_block::{Deserializable, GetRepresentationHash};

use super::{InternalMessage, ONE_EVER};
use crate::network::Subscription;
use crate::util::{make_default_headers, TransactionWithHash};

pub struct Wallet {
    keypair: ed25519_dalek::Keypair,
    address: ton_block::MsgAddressInt,
    subscription: Arc<Subscription>,
}

impl Wallet {
    pub const INITIAL_BALANCE: u128 = 10 * ONE_EVER;

    pub fn new(
        workchain_id: i8,
        keypair: ed25519_dalek::Keypair,
        subscription: Arc<Subscription>,
    ) -> Self {
        Self {
            address: compute_wallet_address(workchain_id, &keypair.public),
            keypair,
            subscription,
        }
    }

    pub fn address(&self) -> &ton_block::MsgAddressInt {
        &self.address
    }

    pub async fn get_balance(&self) -> Result<Option<u128>> {
        let account = self.get_account_state().await?;
        Ok(account.map(|state| state.storage.balance.grams.0))
    }

    /// Sends the internal message to the recipient, returns the destination transaction
    pub async fn call(&self, internal_message: InternalMessage) -> Result<TransactionWithHash> {
        let dst = internal_message.dst.clone();
        let mut dst_transactions = self.subscription.subscribe(&dst);

        let src_tx = self.transfer(internal_message).await?;
        tracing::debug!(source_tx_hash = ?src_tx.hash, "message sent from wallet");

        let mut out_msg_hash = None;
        src_tx
            .data
            .out_msgs
            .iterate_slices(|msg| {
                let Some(msg) = msg.reference_opt(0) else { return Ok(true) };

                let msg_hash = msg.repr_hash();
                let msg = ton_block::Message::construct_from_cell(msg)?;
                let Some(header) = msg.int_header() else { return Ok(true) };

                if header.dst == dst {
                    out_msg_hash = Some(msg_hash);
                    Ok(false)
                } else {
                    Ok(true)
                }
            })
            .context("failed to find outgoing message")?;
        let out_msg_hash = out_msg_hash.context("outgoing message not found")?;

        while let Some(tx) = dst_transactions.recv().await {
            tracing::debug!(source_tx_hash = ?src_tx.hash, tx_hash = ?tx.hash, "new transaction found");
            let Some(msg) = tx.data.in_msg_cell() else { continue; };
            if msg.repr_hash() == out_msg_hash {
                return Ok(tx);
            }
        }
        anyhow::bail!("destination transaction was not found")
    }

    /// Sends the internal message to the recipient, returns the source transaction
    pub async fn transfer(&self, internal_message: InternalMessage) -> Result<TransactionWithHash> {
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
            value: internal_message.amount,
            bounce: false,
            flags: 3,
            payload: internal_message.payload,
        }
        .pack();

        let tx = self
            .subscription
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

        Ok(tx)
    }

    async fn get_account_state(&self) -> Result<Option<ton_block::AccountStuff>> {
        self.subscription
            .get_account_state(&self.address)
            .await
            .context("failed to get account state")
    }
}

pub fn compute_wallet_address(
    workchain_id: i8,
    pubkey: &ed25519_dalek::PublicKey,
) -> ton_block::MsgAddressInt {
    let hash = make_state_init(pubkey)
        .and_then(|state| state.hash())
        .unwrap();
    ton_block::MsgAddressInt::AddrStd(ton_block::MsgAddrStd::with_address(
        None,
        workchain_id,
        hash.into(),
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
                .pubkey_header()
                .time_header()
                .expire_header()
                .inputs(SendTransactionInputs::param_type())
                .build()
        })
    }
}
