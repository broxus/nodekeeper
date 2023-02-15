use std::str::FromStr;

use anyhow::Result;
use hmac::digest::Digest;
use rand::Rng;

mod bip39;
mod legacy;

const LANGUAGE: ::bip39::Language = ::bip39::Language::English;

pub const DEFAULT_PATH: &str = "m/44'/396'/0'/0/0";

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum MnemonicType {
    /// Phrase with 24 words, used in Crystal Wallet
    Legacy,
    /// Phrase with 12 words, used everywhere else. The additional parameter is used in
    /// derivation path to create multiple keys from one mnemonic
    Bip39,
}

impl FromStr for MnemonicType {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "legacy" => Ok(Self::Legacy),
            "bip39" => Ok(Self::Bip39),
            _ => Err(anyhow::anyhow!(
                "unknown mnemonic type (neither `legacy` nor `bip39`)"
            )),
        }
    }
}

pub fn validate_phrase(phrase: &str, mnemonic_type: MnemonicType) -> Result<()> {
    match mnemonic_type {
        MnemonicType::Legacy => legacy::validate_phrase(phrase),
        MnemonicType::Bip39 => bip39::validate_phrase(phrase),
    }
}

pub fn derive_from_phrase(
    phrase: &str,
    mnemonic_type: MnemonicType,
    path: &str,
) -> Result<ed25519_dalek::Keypair> {
    match mnemonic_type {
        MnemonicType::Legacy => legacy::derive_from_phrase(phrase),
        MnemonicType::Bip39 => bip39::derive_from_phrase(phrase, path),
    }
}

/// Generates seed phrase
pub fn generate_seed(mnemonic_type: MnemonicType) -> String {
    use ::bip39::util::{Bits11, IterExt};

    let rng = &mut rand::thread_rng();

    pub fn generate_words(entropy: &[u8]) -> Vec<&'static str> {
        let wordlist = LANGUAGE.wordlist();

        let checksum_byte = sha2::Sha256::digest(entropy)[0];

        entropy
            .iter()
            .chain(Some(&checksum_byte))
            .bits()
            .map(|bits: Bits11| wordlist.get_word(bits))
            .collect()
    }

    match mnemonic_type {
        MnemonicType::Legacy => {
            let entropy: [u8; 32] = rng.gen();
            generate_words(&entropy)
        }
        MnemonicType::Bip39 => {
            let entropy: [u8; 16] = rng.gen();
            generate_words(&entropy)
        }
    }
    .join(" ")
}
