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

engine.validator.generateKeyPair = engine.validator.KeyHash;
engine.validator.exportPublicKey key_hash:int256 = PublicKey;
engine.validator.sign key_hash:int256 data:bytes = engine.validator.Signature;
engine.validator.addValidatorPermanentKey key_hash:int256 election_date:int ttl:int = engine.validator.Success;
engine.validator.addValidatorTempKey permanent_key_hash:int256 key_hash:int256 ttl:int = engine.validator.Success;
engine.validator.addValidatorAdnlAddress permanent_key_hash:int256 key_hash:int256 ttl:int = engine.validator.Success;
engine.validator.addAdnlId key_hash:int256 category:int = engine.validator.Success;
engine.validator.getStats = engine.validator.Stats;
engine.validator.setStatesGcInterval interval_ms:int = engine.validator.Success;

engine.validator.getBundle block_id:tonNode.blockIdExt = engine.validator.Success;
engine.validator.getFutureBundle prev_block_ids:(vector tonNode.blockIdExt) = engine.validator.Success;

liteServer.sendMessage body:bytes = liteServer.SendMsgStatus;
liteServer.getConfigAll mode:# id:tonNode.blockIdExt = liteServer.ConfigInfo;
liteServer.getConfigParams mode:# id:tonNode.blockIdExt param_list:(vector int) = liteServer.ConfigInfo;

raw.getShardAccountState account_address:accountAddress = raw.ShardAccountState;
