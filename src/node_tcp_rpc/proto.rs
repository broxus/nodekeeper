use tl_proto::{IntermediateBytes, TlRead, TlWrite};

#[derive(TlWrite)]
#[tl(boxed, id = "engine.validator.controlQuery", scheme = "proto.tl")]
pub struct ControlQuery<T>(pub IntermediateBytes<T>);

#[derive(Copy, Clone, TlWrite)]
#[tl(boxed, id = "engine.validator.generateKeyPair", scheme = "proto.tl")]
pub struct GenerateKeyPair;

#[derive(Copy, Clone, TlWrite)]
#[tl(boxed, id = "engine.validator.exportPublicKey", scheme = "proto.tl")]
pub struct ExportPublicKey<'tl> {
    #[tl(size_hint = 32)]
    pub key_hash: HashRef<'tl>,
}

#[derive(Copy, Clone, TlWrite)]
#[tl(boxed, id = "engine.validator.sign", scheme = "proto.tl")]
pub struct Sign<'tl> {
    #[tl(size_hint = 32)]
    pub key_hash: HashRef<'tl>,
    pub data: &'tl [u8],
}

#[derive(Copy, Clone, TlWrite)]
#[tl(
    boxed,
    id = "engine.validator.addValidatorPermanentKey",
    scheme = "proto.tl"
)]
pub struct AddValidatorPermanentKey<'tl> {
    #[tl(size_hint = 32)]
    pub key_hash: HashRef<'tl>,
    #[tl(size_hint = 4)]
    pub election_date: u32,
    #[tl(size_hint = 4)]
    pub ttl: u32,
}

#[derive(Copy, Clone, TlWrite)]
#[tl(
    boxed,
    id = "engine.validator.addValidatorAdnlAddress",
    scheme = "proto.tl"
)]
pub struct AddValidatorAdnlAddress<'tl> {
    #[tl(size_hint = 32)]
    pub permanent_key_hash: HashRef<'tl>,
    #[tl(size_hint = 32)]
    pub key_hash: HashRef<'tl>,
    #[tl(size_hint = 4)]
    pub ttl: u32,
}

#[derive(Copy, Clone, TlWrite)]
#[tl(boxed, id = "engine.validator.getStats", scheme = "proto.tl")]
pub struct GetStats;

#[derive(Copy, Clone, TlWrite)]
#[tl(
    boxed,
    id = "engine.validator.setStatesGcInterval",
    scheme = "proto.tl"
)]
pub struct SetStatesGcInterval {
    pub interval_ms: u32,
}

#[derive(Copy, Clone, TlWrite)]
#[tl(boxed, id = "liteServer.sendMessage", scheme = "proto.tl")]
pub struct SendMessage<'tl> {
    pub body: &'tl [u8],
}

#[derive(Copy, Clone, TlWrite)]
#[tl(boxed, id = "liteServer.getConfigAll", scheme = "proto.tl")]
pub struct GetConfigAll<'tl> {
    pub mode: u32,
    pub id: BlockIdExt<'tl>,
}

#[derive(Clone, TlWrite)]
#[tl(boxed, id = "liteServer.getConfigParams", scheme = "proto.tl")]
pub struct GetConfigParams<'tl> {
    pub mode: u32,
    pub id: BlockIdExt<'tl>,
    pub param_list: &'tl [u32],
}

#[derive(Copy, Clone, TlWrite)]
#[tl(boxed, id = "raw.getShardAccountState", scheme = "proto.tl")]
pub struct GetShardAccountState<'tl> {
    pub address: &'tl [u8],
}

#[derive(Copy, Clone, TlRead, TlWrite)]
#[tl(size_hint = 80)]
pub struct BlockIdExt<'tl> {
    pub workchain: i32,
    pub shard: u64,
    pub seqno: u32,
    pub root_hash: HashRef<'tl>,
    pub file_hash: HashRef<'tl>,
}

impl Default for BlockIdExt<'static> {
    fn default() -> Self {
        Self {
            workchain: 0,
            shard: 0x8000000000000000,
            seqno: 0,
            root_hash: &[0; 32],
            file_hash: &[0; 32],
        }
    }
}

#[derive(Copy, Clone, Debug, TlRead, TlWrite)]
#[tl(size_hint = 80)]
pub struct BlockIdExtOwned {
    pub workchain: i32,
    pub shard: u64,
    pub seqno: u32,
    pub root_hash: [u8; 32],
    pub file_hash: [u8; 32],
}

#[derive(Copy, Clone, TlRead)]
#[tl(boxed, id = "engine.validator.success", scheme = "proto.tl")]
pub struct Success;

#[derive(Copy, Clone, TlRead)]
#[tl(boxed, id = "engine.validator.keyHash", scheme = "proto.tl")]
pub struct KeyHash {
    pub key_hash: [u8; 32],
}

#[derive(Clone, TlRead)]
#[tl(boxed, id = "engine.validator.signature", scheme = "proto.tl")]
pub struct Signature {
    pub signature: Vec<u8>,
}

#[derive(Clone, TlRead)]
#[tl(boxed, id = "engine.validator.stats", scheme = "proto.tl")]
pub struct Stats {
    pub items: Vec<OneState>,
}

#[derive(Clone, TlRead)]
pub struct OneState {
    pub key: Vec<u8>,
    pub value: Vec<u8>,
}

#[derive(Clone, Debug, TlRead)]
#[tl(boxed, id = "liteServer.configInfo", scheme = "proto.tl")]
pub struct ConfigInfo {
    pub mode: u32,
    pub id: BlockIdExtOwned,
    pub state_proof: Vec<u8>,
    pub config_proof: Vec<u8>,
}

#[derive(Clone, TlRead)]
#[tl(boxed, scheme = "proto.tl")]
pub enum ShardAccount {
    #[tl(id = "raw.shardAccountState")]
    State(Vec<u8>),
    #[tl(id = "raw.shardAccountNone")]
    Empty,
}

pub type HashRef<'tl> = &'tl [u8; 32];
