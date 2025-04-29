pub mod p2p_server;
pub mod tcp_client;
pub mod tcp_server;

use std::time::{SystemTime, UNIX_EPOCH};

use borsh::{BorshDeserialize, BorshSerialize};
use bytes::BytesMut;
use tcp_client::TcpClient;
use tokio::task::JoinHandle;
use tokio_util::codec::{Decoder, Encoder, LengthDelimitedCodec};

use anyhow::Result;

pub fn get_current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("Time went backwards")
        .as_secs()
}

#[derive(Debug, Clone, BorshDeserialize, BorshSerialize, PartialEq)]
pub enum TcpMessage<Data: Clone> {
    Ping,
    Data(Data),
}

#[derive(Debug, Clone, BorshDeserialize, BorshSerialize, PartialEq)]
pub enum P2PTcpMessage<Data: Clone> {
    Handshake(Handshake),
    Data(Data),
}

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize, PartialEq)]
pub enum Handshake {
    Hello((Canal, sdk::SignedByValidator<NodeConnectionData>, u128)),
    Verack((Canal, sdk::SignedByValidator<NodeConnectionData>, u128)),
}

#[derive(
    Default, Debug, Clone, BorshSerialize, BorshDeserialize, Hash, PartialEq, Eq, PartialOrd, Ord,
)]
pub struct Canal(String);

impl Canal {
    pub fn new<T: Into<String>>(t: T) -> Canal {
        Canal(t.into())
    }
}

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize, PartialEq)]
pub struct NodeConnectionData {
    pub version: u16,
    pub name: String,
    pub p2p_public_address: String,
    pub da_public_address: String,
    // TODO: add known peers
    // pub peers: Vec<String>, // List of known peers
}

#[derive(Debug, Clone, BorshDeserialize, BorshSerialize)]
pub enum TcpEvent<Data: Clone> {
    Message { dest: String, data: Data },
    Error { dest: String, error: String },
    Closed { dest: String },
}

#[derive(Debug)]
pub enum P2PTcpEvent<Codec, Msg>
where
    Msg: Clone + std::fmt::Debug,
    Codec:
        Decoder<Item = P2PTcpMessage<Msg>> + Encoder<P2PTcpMessage<Msg>> + Default + Send + 'static,
    <Codec as Decoder>::Error: std::fmt::Debug + Send,
    <Codec as Encoder<P2PTcpMessage<Msg>>>::Error: std::fmt::Debug + Send,
{
    TcpEvent(TcpEvent<P2PTcpMessage<Msg>>),
    HandShakeTcpClient(
        TcpClient<Codec, P2PTcpMessage<Msg>, P2PTcpMessage<Msg>>,
        Canal,
    ),
    PingPeers,
}

// A Generic Codec to unwrap/wrap with TcpMessage<T>
#[derive(Debug)]
pub struct TcpMessageCodec<T> {
    _marker: std::marker::PhantomData<T>,
    ldc: LengthDelimitedCodec,
}

impl<T> Default for TcpMessageCodec<T> {
    fn default() -> Self {
        let ldc = LengthDelimitedCodec::default();
        Self {
            _marker: std::marker::PhantomData,
            ldc,
        }
    }
}

impl<Codec> TcpMessageCodec<Codec> {
    fn new(max_frame_length: usize) -> TcpMessageCodec<Codec> {
        let mut ldc = LengthDelimitedCodec::default();
        ldc.set_max_frame_length(max_frame_length);
        Self {
            _marker: std::marker::PhantomData,
            ldc,
        }
    }
}

impl<Codec, Decodable> Decoder for TcpMessageCodec<Codec>
where
    Codec: Decoder<Item = Decodable> + Send,
    Decodable: BorshDeserialize + Clone,
{
    type Item = TcpMessage<Decodable>;
    type Error = std::io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if src.is_empty() {
            return Ok(None);
        }

        let Some(src_ldc) = self.ldc.decode(src)? else {
            return Ok(None);
        };

        let msg: TcpMessage<Decodable> = borsh::from_slice(&src_ldc[..]).map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Failed to decode TcpMessage: {}", e),
            )
        })?;

        Ok(Some(msg))
    }
}

impl<Codec, Encodable> Encoder<TcpMessage<Encodable>> for TcpMessageCodec<Codec>
where
    Codec: Encoder<Encodable> + Send,
    Encodable: BorshSerialize + Clone,
{
    type Error = std::io::Error;

    fn encode(
        &mut self,
        item: TcpMessage<Encodable>,
        dst: &mut BytesMut,
    ) -> Result<(), Self::Error> {
        let serialized = borsh::to_vec(&item).map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("Failed to encode TcpMessage: {}", e),
            )
        })?;
        self.ldc.encode(serialized.into(), dst).map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("Failed to encode with LengthDelimitedCodec: {}", e),
            )
        })?;

        Ok(())
    }
}

/// A socket abstraction to send a receive data
#[derive(Debug)]
struct SocketStream<Out>
where
    Out: Clone + std::fmt::Debug,
{
    /// Last timestamp we received a ping from the peer.
    last_ping: u64,
    /// Sender to stream data to the peer
    sender: tokio::sync::mpsc::Sender<TcpMessage<Out>>,
    /// Handle to abort the sending side of the stream
    abort_sender_task: JoinHandle<()>,
    /// Handle to abort the receiving side of the stream
    abort_receiver_task: JoinHandle<()>,
}

#[macro_export]
macro_rules! implem_tcp_codec {
    ($codec:ident, decode: $in:ty, encode: $out:ty) => {
        #[derive(Default, Debug)]
        pub struct $codec;

        impl tokio_util::codec::Encoder<$out> for $codec {
            type Error = anyhow::Error;

            fn encode(
                &mut self,
                event: $out,
                dst: &mut bytes::BytesMut,
            ) -> Result<(), Self::Error> {
                let bytes: Vec<u8> = borsh::to_vec(&event)?;
                bytes::BufMut::put_slice(dst, bytes.as_slice());
                Ok(())
            }
        }

        impl tokio_util::codec::Decoder for $codec {
            type Item = $in;
            type Error = anyhow::Error;

            fn decode(
                &mut self,
                src: &mut bytes::BytesMut,
            ) -> Result<Option<Self::Item>, Self::Error> {
                Ok(Some(
                    borsh::from_slice(&src).context(format!("Decoding bytes with borsh",))?,
                ))
            }
        }
    };
}

pub use implem_tcp_codec;

#[macro_export]
macro_rules! tcp_client_server {
    ($vis:vis $name:ident, request: $req:ty, response: $res:ty) => {
        paste::paste! {
        $vis mod [< codec_ $name:snake >] {
            #![allow(unused)]
            use anyhow::{Context, Result};
            $crate::tcp::implem_tcp_codec!{
                ClientCodec,
                decode: $res,
                encode: $req
            }
            $crate::tcp::implem_tcp_codec!{
                ServerCodec,
                decode: $req,
                encode: $res
            }

            pub type Client = $crate::tcp::tcp_client::TcpClient<ClientCodec, $req, $res>;
            pub type Server = $crate::tcp::tcp_server::TcpServer<ServerCodec, $req, $res>;
            pub async fn start_server(port: u16) -> Result<Server> {
                $crate::tcp::tcp_server::TcpServer::<ServerCodec, $req, $res>::start_with_opts(port, None, stringify!($name).to_string()).await
            }
            pub async fn start_server_with_opts(port: u16, max_frame_len: Option<usize>) -> Result<Server> {
                $crate::tcp::tcp_server::TcpServer::<ServerCodec, $req, $res>::start_with_opts(port, max_frame_len, stringify!($name).to_string()).await
            }
            pub async fn connect<Id: std::fmt::Display, A: $crate::net::ToSocketAddrs + std::fmt::Display>(id: Id, addr: A) -> Result<Client> {
                $crate::tcp::tcp_client::TcpClient::<ClientCodec, $req, $res>::connect(id, addr).await
            }
            pub async fn connect_with_opts<Id: std::fmt::Display, A: $crate::net::ToSocketAddrs + std::fmt::Display>(id: Id, max_frame_length: Option<usize>, addr: A) -> Result<Client> {
                $crate::tcp::tcp_client::TcpClient::<ClientCodec, $req, $res>::connect_with_opts(id, max_frame_length, addr).await
            }
        }
        }
    };
}

pub use tcp_client_server;

#[macro_export]
macro_rules! p2p_server_mod {
    ($vis:vis $name:ident, message: $msg:ty) => {
        paste::paste! {
        $vis mod [< p2p_server_ $name:snake >] {

            $crate::tcp_client_server!{
                $vis tcp,
                request: $crate::tcp::P2PTcpMessage<$msg>,
                response: $crate::tcp::P2PTcpMessage<$msg>
            }

            type P2PServerType = $crate::tcp::p2p_server::P2PServer<codec_tcp::ServerCodec, $msg>;

            pub async fn start_server(
                crypto: std::sync::Arc<hyle_crypto::BlstCrypto>,
                node_id: String,
                server_port: u16,
                max_frame_length: Option<usize>,
                node_p2p_public_adress: String,
                node_da_public_adress: String,
            ) -> anyhow::Result<P2PServerType> {
                let server = codec_tcp::start_server_with_opts(server_port, max_frame_length).await?;

                Ok(
                    $crate::tcp::p2p_server::P2PServer::new(
                        crypto,
                        node_id,
                        max_frame_length,
                        node_p2p_public_adress,
                        node_da_public_adress,
                        server
                    )
                )

            }
        }
        }
    };
}

pub use p2p_server_mod;
