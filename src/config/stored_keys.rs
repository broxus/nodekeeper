use std::path::Path;

use anyhow::{Context, Result};
use broxus_util::{serde_hex_array, serde_optional_hex_array};
use serde::{Deserialize, Serialize};

use crate::crypto::*;

#[derive(Serialize)]
pub struct StoredKeys {
    #[serde(with = "serde_hex_array")]
    pub secret: [u8; 32],
    #[serde(
        with = "serde_optional_hex_array",
        skip_serializing_if = "Option::is_none"
    )]
    pub public: Option<[u8; 32]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed: Option<String>,
}

impl StoredKeys {
    pub const DEFAULT_MNEMONIC_TYPE: MnemonicType = MnemonicType::Bip39;

    pub fn generate() -> Result<Self> {
        Self::from_seed(generate_seed(Self::DEFAULT_MNEMONIC_TYPE))
    }

    pub fn from_seed<T: AsRef<str>>(seed: T) -> Result<Self> {
        fn inner(seed: &str) -> Result<StoredKeys> {
            let seed = seed.trim().to_owned();
            let keypair =
                derive_from_phrase(&seed, StoredKeys::DEFAULT_MNEMONIC_TYPE, DEFAULT_PATH)?;
            Ok(StoredKeys {
                secret: keypair.secret.to_bytes(),
                public: Some(keypair.public.to_bytes()),
                seed: Some(seed),
            })
        }

        inner(seed.as_ref())
    }

    pub fn load_as_keypair<P: AsRef<Path>>(path: P) -> Result<ed25519_dalek::Keypair> {
        Ok(Self::load(path)?.as_keypair())
    }

    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self> {
        fn inner(path: &Path) -> Result<StoredKeys> {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            pub struct StoredKeysHelper {
                #[serde(with = "serde_optional_hex_array")]
                pub secret: Option<[u8; 32]>,
                #[serde(default, with = "serde_optional_hex_array")]
                pub public: Option<[u8; 32]>,
                #[serde(default)]
                pub seed: Option<String>,
            }

            let file = std::fs::File::open(path).context("failed to open keys file")?;
            let mut deserializer =
                serde_json::Deserializer::from_reader(std::io::BufReader::new(file));
            let data: StoredKeysHelper = serde_path_to_error::deserialize(&mut deserializer)
                .context("failed to parse keys")?;

            if let Some(secret) = data.secret {
                Ok(StoredKeys {
                    secret,
                    public: data.public,
                    seed: data.seed,
                })
            } else if let Some(seed) = data.seed {
                StoredKeys::from_seed(seed)
            } else {
                anyhow::bail!("invalid keys file")
            }
        }

        inner(path.as_ref())
    }

    pub fn store<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let data = serde_json::to_string_pretty(self).context("failed to serialize keys")?;
        std::fs::write(path, data).context("failed to save keys")
    }

    pub fn as_secret(&self) -> ed25519_dalek::SecretKey {
        ed25519_dalek::SecretKey::from_bytes(&self.secret).unwrap()
    }

    pub fn as_keypair(&self) -> ed25519_dalek::Keypair {
        let secret = self.as_secret();
        let public = ed25519_dalek::PublicKey::from(&secret);
        ed25519_dalek::Keypair { secret, public }
    }
}
