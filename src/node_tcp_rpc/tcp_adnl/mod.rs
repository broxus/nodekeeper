use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use ctr::cipher::{KeyIvInit, StreamCipher};
use everscale_crypto::ed25519;
use rand::{Rng, RngCore};
use sha2::Digest;
use tl_proto::{IntermediateBytes, TlRead, TlWrite};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use self::queries_cache::QueriesCache;

mod queries_cache;

pub struct TcpAdnlConfig {
    pub server_address: SocketAddr,
    pub server_pubkey: ed25519::PublicKey,
    pub client_secret: ed25519::SecretKey,
    pub connection_timeout: Duration,
}

#[derive(Clone)]
pub struct TcpAdnl {
    state: Arc<SharedState>,
}

impl TcpAdnl {
    pub async fn connect(config: TcpAdnlConfig) -> Result<Self, TcpAdnlError> {
        let (socket_rx, socket_tx) = match tokio::time::timeout(
            config.connection_timeout,
            TcpStream::connect(config.server_address),
        )
        .await
        {
            Ok(connection) => connection
                .map_err(TcpAdnlError::ConnectionError)?
                .into_split(),
            Err(_) => return Err(TcpAdnlError::ConnectionTimeout),
        };

        let mut initial_buffer = vec![0; 160];
        rand::thread_rng().fill_bytes(&mut initial_buffer);

        let cipher_receive = Aes256Ctr::new(
            generic_array::GenericArray::from_slice(&initial_buffer[0..32]),
            generic_array::GenericArray::from_slice(&initial_buffer[64..80]),
        );
        let cipher_send = Aes256Ctr::new(
            generic_array::GenericArray::from_slice(&initial_buffer[32..64]),
            generic_array::GenericArray::from_slice(&initial_buffer[80..96]),
        );

        let (tx, rx) = mpsc::unbounded_channel::<Packet>();

        let state = Arc::new(SharedState {
            queries_cache: Arc::new(Default::default()),
            cancellation_token: Default::default(),
            packets_tx: tx,
            query_id: Default::default(),
        });

        tokio::spawn(socket_writer(
            socket_tx,
            cipher_send,
            rx,
            state.cancellation_token.clone(),
        ));
        tokio::spawn(socket_reader(
            socket_rx,
            cipher_receive,
            state.queries_cache.clone(),
            state.cancellation_token.clone(),
        ));

        build_handshake_packet(
            &config.server_pubkey,
            &config.client_secret,
            &mut initial_buffer,
        );
        state
            .packets_tx
            .send(Packet::unencrypted(initial_buffer))
            .ok()
            .unwrap();

        Ok(Self { state })
    }

    pub async fn query<Q, R>(&self, query: Q, timeout: Duration) -> Result<Option<R>, TcpAdnlError>
    where
        Q: TlWrite<Repr = tl_proto::Boxed>,
        for<'a> R: TlRead<'a>,
    {
        let cancelled = self.state.cancellation_token.cancelled();
        if self.state.cancellation_token.is_cancelled() {
            return Err(TcpAdnlError::SocketClosed);
        }

        let mut query_id = [0; 32];
        query_id[..std::mem::size_of::<usize>()].copy_from_slice(
            &self
                .state
                .query_id
                .fetch_add(1, Ordering::AcqRel)
                .to_le_bytes(),
        );

        let data = tl_proto::serialize(AdnlMessageQuery {
            query_id: &query_id,
            query: IntermediateBytes(query),
        });

        let pending_query = self.state.queries_cache.add_query(query_id);
        if self.state.packets_tx.send(Packet::encrypted(data)).is_err() {
            return Err(TcpAdnlError::SocketClosed);
        }

        let answer = tokio::select! {
            res = tokio::time::timeout(timeout, pending_query.wait()) => {
                res.ok().flatten()
            }
            _  = cancelled => return Err(TcpAdnlError::SocketClosed),
        };

        Ok(match answer {
            Some(query) => {
                Some(tl_proto::deserialize(&query).map_err(TcpAdnlError::InvalidAnswer)?)
            }
            None => None,
        })
    }
}

struct SharedState {
    queries_cache: Arc<QueriesCache>,
    cancellation_token: CancellationToken,
    packets_tx: PacketsTx,
    query_id: AtomicUsize,
}

impl Drop for SharedState {
    fn drop(&mut self) {
        self.cancellation_token.cancel();
    }
}

async fn socket_writer<T>(
    mut socket: T,
    mut cipher: Aes256Ctr,
    mut rx: PacketsRx,
    cancellation_token: CancellationToken,
) where
    T: AsyncWrite + Unpin,
{
    tokio::pin!(let cancelled = cancellation_token.cancelled(););

    while let Some(mut packet) = rx.recv().await {
        let data = &mut packet.data;

        if packet.encrypt {
            let len = data.len();

            data.reserve(len + 68);
            data.resize(len + 36, 0);
            data.copy_within(..len, 36);
            data[..4].copy_from_slice(&((len + 64) as u32).to_le_bytes());

            let nonce: [u8; 32] = rand::thread_rng().gen();
            data[4..36].copy_from_slice(&nonce);

            data.extend_from_slice(sha2::Sha256::digest(&data[4..]).as_slice());

            cipher.apply_keystream(data);
        }

        tokio::select! {
            res = socket.write_all(data) => match res {
                Ok(_) => continue,
                Err(e) => {
                    if !cancellation_token.is_cancelled() {
                        cancellation_token.cancel();
                        tracing::error!("failed to write data to the socket: {e:?}");
                    }
                    break;
                }
            },
            _ = &mut cancelled => break,
        }
    }

    tracing::debug!("sender loop finished");
}

async fn socket_reader<T>(
    mut socket: T,
    mut cipher: Aes256Ctr,
    queries_cache: Arc<QueriesCache>,
    cancellation_token: CancellationToken,
) where
    T: AsyncRead + Unpin,
{
    tokio::pin!(let cancelled = cancellation_token.cancelled(););

    loop {
        let mut length = [0; 4];
        tokio::select! {
            res = socket.read_exact(&mut length) => match res {
                Ok(_) => cipher.apply_keystream(&mut length),
                Err(e) => {
                    if !cancellation_token.is_cancelled() {
                        cancellation_token.cancel();
                        tracing::error!("failed to read length from the socket: {e:?}");
                    }
                    break;
                }
            },
            _ = &mut cancelled => break,
        }

        let length = u32::from_le_bytes(length) as usize;
        if length < 64 {
            continue;
        }

        let mut buffer = vec![0; length];
        tokio::select! {
            res = socket.read_exact(&mut buffer) => match res {
                Ok(_) => cipher.apply_keystream(&mut buffer),
                Err(e) => {
                    if !cancellation_token.is_cancelled() {
                        cancellation_token.cancel();
                        tracing::error!("failed to read data from the socket: {e:?}");
                    }
                    break;
                }
            },
            _ = &mut cancelled => break,
        }

        if !sha2::Sha256::digest(&buffer[..length - 32])
            .as_slice()
            .eq(&buffer[length - 32..length])
        {
            tracing::warn!("packet checksum mismatch");
            continue;
        }

        buffer.truncate(length - 32);
        buffer.drain(..32);

        if buffer.is_empty() {
            continue;
        }

        match tl_proto::deserialize::<AdnlMessageAnswer>(&buffer) {
            Ok(AdnlMessageAnswer { query_id, data }) => {
                queries_cache.update_query(query_id, data);
            }
            Err(e) => tracing::warn!("invalid response: {e:?}"),
        };
    }

    tracing::debug!("receiver loop finished");
}

struct Packet {
    data: Vec<u8>,
    encrypt: bool,
}

impl Packet {
    fn encrypted(data: Vec<u8>) -> Self {
        Self {
            data,
            encrypt: true,
        }
    }

    fn unencrypted(data: Vec<u8>) -> Self {
        Self {
            data,
            encrypt: false,
        }
    }
}

type PacketsTx = mpsc::UnboundedSender<Packet>;
type PacketsRx = mpsc::UnboundedReceiver<Packet>;

pub fn build_handshake_packet(
    server_pubkey: &ed25519::PublicKey,
    client_secret: &ed25519::SecretKey,
    buffer: &mut Vec<u8>,
) {
    let server_short_id = tl_proto::hash(server_pubkey.as_tl());
    let client_public_key = ed25519::PublicKey::from(client_secret);

    let shared_secret = client_secret.expand().compute_shared_secret(server_pubkey);

    // Prepare packet
    let checksum: [u8; 32] = sha2::Sha256::digest(buffer.as_slice()).into();

    let length = buffer.len();
    buffer.resize(length + 96, 0);
    buffer.copy_within(..length, 96);

    buffer[..32].copy_from_slice(server_short_id.as_slice());
    buffer[32..64].copy_from_slice(client_public_key.as_bytes());
    buffer[64..96].copy_from_slice(&checksum);

    // Encrypt packet data
    build_packet_cipher(&shared_secret, &checksum).apply_keystream(&mut buffer[96..]);
}

pub fn build_packet_cipher(shared_secret: &[u8; 32], checksum: &[u8; 32]) -> Aes256Ctr {
    let mut aes_key_bytes: [u8; 32] = *shared_secret;
    aes_key_bytes[16..32].copy_from_slice(&checksum[16..32]);
    let mut aes_ctr_bytes: [u8; 16] = checksum[0..16].try_into().unwrap();
    aes_ctr_bytes[4..16].copy_from_slice(&shared_secret[20..32]);

    Aes256Ctr::new(
        &generic_array::GenericArray::from(aes_key_bytes),
        &generic_array::GenericArray::from(aes_ctr_bytes),
    )
}

#[derive(Clone, TlWrite)]
#[tl(boxed, id = "adnl.message.query", scheme = "proto.tl")]
struct AdnlMessageQuery<'tl, T> {
    #[tl(size_hint = 32)]
    query_id: &'tl [u8; 32],
    query: IntermediateBytes<T>,
}

#[derive(Copy, Clone, TlRead)]
#[tl(boxed, id = "adnl.message.answer", scheme = "proto.tl")]
struct AdnlMessageAnswer<'tl> {
    #[tl(size_hint = 32)]
    query_id: &'tl [u8; 32],
    data: &'tl [u8],
}

#[derive(thiserror::Error, Debug)]
pub enum TcpAdnlError {
    #[error("connection timeout")]
    ConnectionTimeout,
    #[error("failed to open connection")]
    ConnectionError(#[source] std::io::Error),
    #[error("socket closed")]
    SocketClosed,
    #[error("invalid answer")]
    InvalidAnswer(#[source] tl_proto::TlError),
}

pub type Aes256Ctr = ctr::Ctr64BE<aes::Aes256>;
