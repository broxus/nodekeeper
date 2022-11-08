use anyhow::Result;
use tiny_hderive::bip32::ExtendedPrivKey;

use super::LANGUAGE;

pub fn validate_phrase(phrase: &str) -> Result<()> {
    bip39::Mnemonic::from_phrase(phrase, LANGUAGE)?;
    Ok(())
}

pub fn derive_from_phrase(phrase: &str, path: &str) -> Result<ed25519_dalek::Keypair> {
    let mnemonic = bip39::Mnemonic::from_phrase(phrase, LANGUAGE)?;
    let hd = bip39::Seed::new(&mnemonic, "");
    let seed_bytes = hd.as_bytes();

    let derived = ExtendedPrivKey::derive(seed_bytes, path)
        .map_err(|_| anyhow::anyhow!("Invalid derivation path"))?;

    let secret = ed25519_dalek::SecretKey::from_bytes(&derived.secret())?;
    let public = ed25519_dalek::PublicKey::from(&secret);
    Ok(ed25519_dalek::Keypair { secret, public })
}
