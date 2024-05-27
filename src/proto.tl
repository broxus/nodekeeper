// Generic stuff
////////////////////////////////////////////////////////////////////////////////

---types---

int ? = Int;
long ? = Long;
string ? = String;
object ? = Object;
int256 8*[ int ] = Int256;
bytes data:string = Bytes;

pub.unenc data:bytes = PublicKey;
pub.ed25519 key:int256 = PublicKey;
pub.aes key:int256 = PublicKey;
pub.overlay name:bytes = PublicKey;


// Node TCP ADNL stuff
////////////////////////////////////////////////////////////////////////////////

---types---

tcp.pong random_id:long = tcp.Pong;

adnl.message.query query_id:int256 query:bytes = adnl.Message;
adnl.message.answer query_id:int256 answer:bytes = adnl.Message;

tonNode.blockIdExt workchain:int shard:long seqno:int root_hash:int256 file_hash:int256 = tonNode.BlockIdExt;

accountAddress account_address:string = AccountAddress;

engine.validator.success = engine.validator.Success;

engine.validator.keyHash key_hash:int256 = engine.validator.KeyHash;
engine.validator.signature signature:bytes = engine.validator.Signature;

engine.validator.oneStat key:string value:string = engine.validator.OneStat;
engine.validator.stats stats:(vector engine.validator.oneStat) = engine.validator.Stats;

liteServer.sendMsgStatus status:int = liteServer.SendMsgStatus;
liteServer.configInfo mode:# id:tonNode.blockIdExt state_proof:bytes config_proof:bytes = liteServer.ConfigInfo;

raw.shardAccountState shard_account:bytes = raw.ShardAccountState;
raw.shardAccountNone = raw.ShardAccountState;

---functions---

tcp.ping random_id:long = tcp.Pong;

engine.validator.controlQuery data:bytes = Object;
engine.validator.controlQueryError code:int message:string = engine.validator.ControlQueryError;

engine.validator.generateKeyPair = engine.validator.KeyHash;
engine.validator.exportPublicKey key_hash:int256 = PublicKey;
engine.validator.sign key_hash:int256 data:bytes = engine.validator.Signature;
engine.validator.addValidatorPermanentKey key_hash:int256 election_date:int ttl:int = engine.validator.Success;
engine.validator.addValidatorAdnlAddress permanent_key_hash:int256 key_hash:int256 ttl:int = engine.validator.Success;
engine.validator.getStats = engine.validator.Stats;
engine.validator.setStatesGcInterval interval_ms:int = engine.validator.Success;

liteServer.sendMessage body:bytes = liteServer.SendMsgStatus;
liteServer.getConfigAll mode:# id:tonNode.blockIdExt = liteServer.ConfigInfo;
liteServer.getConfigParams mode:# id:tonNode.blockIdExt param_list:(vector int) = liteServer.ConfigInfo;

raw.getShardAccountState account_address:accountAddress = raw.ShardAccountState;


// Node UDP ADNL stuff
////////////////////////////////////////////////////////////////////////////////

---types---

tonNode.blockDescriptionEmpty = tonNode.BlockDescription;
tonNode.blockDescription id:tonNode.blockIdExt = tonNode.BlockDescription;

tonNode.preparedProofEmpty = tonNode.PreparedProof;
tonNode.preparedProof = tonNode.PreparedProof;
tonNode.preparedProofLink = tonNode.PreparedProof;

tonNode.prepared = tonNode.Prepared;
tonNode.notFound = tonNode.Prepared;

tonNode.preparedState = tonNode.PreparedState;
tonNode.notFoundState = tonNode.PreparedState;

tonNode.keyBlocks blocks:(vector tonNode.blockIdExt) incomplete:Bool error:Bool = tonNode.KeyBlocks;

tonNode.dataFull id:tonNode.blockIdExt proof:bytes block:bytes is_link:Bool = tonNode.DataFull;
tonNode.dataFullEmpty = tonNode.DataFull;

tonNode.data data:bytes = tonNode.Data;

tonNode.archiveNotFound = tonNode.ArchiveInfo;
tonNode.archiveInfo id:long = tonNode.ArchiveInfo;

tonNode.capabilities version:int capabilities:long = tonNode.Capabilities;

---functions---

tonNode.getNextBlockDescription prev_block:tonNode.blockIdExt = tonNode.BlockDescription;
tonNode.prepareBlockProof block:tonNode.blockIdExt allow_partial:Bool = tonNode.PreparedProof;
tonNode.prepareKeyBlockProof block:tonNode.blockIdExt allow_partial:Bool = tonNode.PreparedProof;
tonNode.prepareBlock block:tonNode.blockIdExt = tonNode.Prepared;
tonNode.getNextKeyBlockIds block:tonNode.blockIdExt max_size:int = tonNode.KeyBlocks;
tonNode.downloadNextBlockFull prev_block:tonNode.blockIdExt = tonNode.DataFull;
tonNode.downloadBlockFull block:tonNode.blockIdExt = tonNode.DataFull;
tonNode.downloadBlock block:tonNode.blockIdExt = tonNode.Data;

tonNode.downloadBlockProof block:tonNode.blockIdExt = tonNode.Data;
tonNode.downloadKeyBlockProof block:tonNode.blockIdExt = tonNode.Data;
tonNode.downloadBlockProofLink block:tonNode.blockIdExt = tonNode.Data;
tonNode.downloadKeyBlockProofLink block:tonNode.blockIdExt = tonNode.Data;

tonNode.getCapabilities = tonNode.Capabilities;
