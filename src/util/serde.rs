use everscale_crypto::ed25519;
use serde::{Deserialize, Deserializer, Serializer};

pub mod serde_public_key {
    use super::*;

    pub fn serialize<S: Serializer>(
        public: &ed25519::PublicKey,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&hex::encode(public.as_bytes()))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<ed25519::PublicKey, D::Error> {
        use serde::de::Error;

        let str = String::deserialize(deserializer)?;
        let bytes = match hex::decode(&str) {
            Ok(bytes) if bytes.len() == 32 => bytes,
            _ => match base64::decode(&str) {
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
    use super::*;

    pub fn serialize<S: Serializer>(
        secret: &ed25519::SecretKey,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&hex::encode(secret.as_bytes()))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<ed25519::SecretKey, D::Error> {
        use serde::de::Error;

        let str = String::deserialize(deserializer)?;
        let bytes = match hex::decode(&str) {
            Ok(bytes) if bytes.len() == 32 => bytes,
            _ => match base64::decode(&str) {
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

pub mod serde_block_id {
    use super::*;

    pub fn serialize<S: Serializer>(
        block_id: &ton_block::BlockIdExt,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&format!(
            "{}:{:016x}:{}:{:x}:{:x}",
            block_id.shard_id.workchain_id(),
            block_id.shard_id.shard_prefix_with_tag(),
            block_id.seq_no,
            block_id.root_hash,
            block_id.file_hash
        ))
    }
}
