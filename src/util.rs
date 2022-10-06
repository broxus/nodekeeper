use std::hash::BuildHasherDefault;

use dashmap::DashMap;

pub type FxDashMap<K, V> = DashMap<K, V, BuildHasherDefault<rustc_hash::FxHasher>>;

pub mod serde_public_key {
    use everscale_crypto::ed25519;
    use serde::de::{Deserialize, Deserializer, Error};

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<ed25519::PublicKey, D::Error> {
        let str = <&str>::deserialize(deserializer)?;
        let bytes = match hex::decode(str) {
            Ok(bytes) if bytes.len() == 32 => bytes,
            _ => match base64::decode(str) {
                Ok(bytes) => bytes,
                Err(_) => return Err(Error::custom("invalid pubkey string")),
            },
        };

        let bytes = bytes
            .try_into()
            .map_err(|_| Error::custom("invalid pubkey length"))?;

        ed25519::PublicKey::from_bytes(bytes).ok_or_else(|| Error::custom("invalid pubkey"))
    }
}

pub mod serde_secret_key {
    use everscale_crypto::ed25519;
    use serde::de::{Deserialize, Deserializer, Error};

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<ed25519::SecretKey, D::Error> {
        let str = <&str>::deserialize(deserializer)?;
        let bytes = match hex::decode(str) {
            Ok(bytes) if bytes.len() == 32 => bytes,
            _ => match base64::decode(str) {
                Ok(bytes) => bytes,
                Err(_) => return Err(Error::custom("invalid secret key string")),
            },
        };

        let bytes = bytes
            .try_into()
            .map_err(|_| Error::custom("invalid secret key length"))?;

        Ok(ed25519::SecretKey::from_bytes(bytes))
    }
}
