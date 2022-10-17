use anyhow::Error;
use ed25519_dalek::Keypair;
use hmac::{Mac, NewMac};
use pbkdf2::pbkdf2;

use super::LANGUAGE;

pub fn derive_from_phrase(phrase: &str) -> Result<Keypair, Error> {
    const PBKDF_ITERATIONS: u32 = 100_000;
    const SALT: &[u8] = b"TON default seed";

    let wordmap = LANGUAGE.wordmap();
    let mut word_count = 0;
    for word in phrase.split_whitespace() {
        word_count += 1;
        if word_count > 24 {
            anyhow::bail!("Expected 24 words")
        }

        wordmap.get_bits(word)?;
    }
    if word_count != 24 {
        anyhow::bail!("Expected 24 words")
    }

    let password = hmac::Hmac::<sha2::Sha512>::new_from_slice(phrase.as_bytes())
        .unwrap()
        .finalize()
        .into_bytes();

    let mut res = [0; 512 / 8];
    pbkdf2::<hmac::Hmac<sha2::Sha512>>(&password, SALT, PBKDF_ITERATIONS, &mut res);

    let secret = ed25519_dalek::SecretKey::from_bytes(&res[0..32])?;
    let public = ed25519_dalek::PublicKey::from(&secret);
    Ok(Keypair { secret, public })
}
