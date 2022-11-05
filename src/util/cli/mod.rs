use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::{Context, Result};
use broxus_util::serde_hex_array;
use serde::Deserialize;
use ton_block::Deserializable;

mod spinner;

pub fn print_output<T: std::fmt::Display>(arg: T) {
    if console::user_attended() {
        writeln!(std::io::stdout(), "{arg:#}")
    } else {
        write!(std::io::stdout(), "{arg}")
    }
    .unwrap()
}

pub fn parse_contract_abi<P>(path: P) -> Result<ton_abi::Contract>
where
    P: AsRef<Path>,
{
    let data = std::fs::read(path.as_ref()).context("failed to read JSON ABI")?;
    let mut jd = serde_json::Deserializer::from_slice(&data);
    let contract: ton_abi::contract::SerdeContract =
        serde_path_to_error::deserialize(&mut jd).context("failed to parse JSON ABI")?;
    ton_abi::Contract::try_from(contract)
}

pub fn parse_address(address: &str) -> Result<ton_block::MsgAddressInt> {
    ton_block::MsgAddressInt::from_str(address).map_err(From::from)
}

pub fn parse_optional_pubkey(pubkey: Option<String>) -> Result<Option<ed25519_dalek::PublicKey>> {
    match pubkey {
        Some(pubkey) => {
            let pubkey = parse_hex_or_base64(&pubkey)?;
            Ok(Some(ed25519_dalek::PublicKey::from_bytes(&pubkey)?))
        }
        None => Ok(None),
    }
}

pub fn parse_optional_input(data: Option<String>, raw: bool) -> Result<Vec<u8>> {
    match data {
        Some(data) if raw => Ok(data.into()),
        Some(data) => parse_hex_or_base64(&data),
        None => {
            let mut data = Vec::new();
            std::io::stdin()
                .read_to_end(&mut data)
                .context("failed to read from stdin")?;
            Ok(data)
        }
    }
}

pub fn parse_key_hash(hash: &str) -> Result<[u8; 32]> {
    let data = parse_hex_or_base64(hash).context("invalid key hash")?;
    data.try_into()
        .map_err(|_| anyhow::Error::msg("invalid key hash length"))
}

pub fn parse_optional_state_init(data: Option<String>) -> Result<Option<ton_block::StateInit>> {
    data.as_deref().map(parse_state_init).transpose()
}

pub fn parse_state_init(data: &str) -> Result<ton_block::StateInit> {
    ton_block::StateInit::construct_from_base64(data)
}

pub fn parse_hex_or_base64(data: &str) -> Result<Vec<u8>> {
    if let Some(hash) = data.strip_prefix("0x") {
        hex::decode(hash).map_err(From::from)
    } else {
        match hex::decode(data) {
            Ok(bytes) => Ok(bytes),
            Err(e) => match base64::decode(data) {
                Ok(bytes) => Ok(bytes),
                _ => Err(e.into()),
            },
        }
    }
}

pub fn parse_keys(keys: Option<PathBuf>) -> Result<Option<ed25519_dalek::Keypair>> {
    #[derive(Deserialize)]
    struct StoredKeyPair {
        #[serde(with = "serde_hex_array")]
        secret: [u8; 32],
    }

    let Some(path) = keys else {
        return Ok(None);
    };

    let keypair = std::fs::read_to_string(path).context("failed to read keys")?;
    let keypair =
        serde_json::from_str::<StoredKeyPair>(&keypair).context("failed to parse keys")?;

    let secret =
        ed25519_dalek::SecretKey::from_bytes(&keypair.secret).context("invalid secret key")?;
    let public = ed25519_dalek::PublicKey::from(&secret);

    Ok(Some(ed25519_dalek::Keypair { secret, public }))
}
