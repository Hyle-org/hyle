pub mod p2p_server;
pub mod tcp_client;
pub mod tcp_server;

use std::time::{SystemTime, UNIX_EPOCH};

use borsh::{BorshDeserialize, BorshSerialize};
use bytes::BytesMut;
use futures::stream::SplitSink;
use tokio::task::JoinHandle;
use tokio_util::codec::{Decoder, Encoder, Framed, LengthDelimitedCodec};

use crate::net::TcpStream;
use anyhow::{Context, Result};

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
    Hello((sdk::SignedByValidator<NodeConnectionData>, u128)),
    Verack((sdk::SignedByValidator<NodeConnectionData>, u128)),
}

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize, PartialEq)]
pub struct NodeConnectionData {
    pub version: u16,
    pub name: String,
    pub hostname: String,
    pub p2p_port: u16,
    pub da_port: u16,
    // TODO: add known peers
    // pub peers: Vec<String>, // List of known peers
}

#[derive(Debug, Clone, BorshDeserialize, BorshSerialize)]
pub enum TcpCommand<Data: Clone> {
    Broadcast(Data),
    Send(String, Data),
}

#[derive(Debug, Clone, BorshDeserialize, BorshSerialize)]
pub struct TcpEvent<Data: Clone> {
    pub dest: String,
    pub data: Data,
}

// A Generic Codec to unwrap/wrap with TcpMessage<T>
#[derive(Debug)]
pub struct TcpMessageCodec<T> {
    _marker: std::marker::PhantomData<T>,
    ldc: LengthDelimitedCodec,
}

impl<T> Default for TcpMessageCodec<T> {
    fn default() -> Self {
        Self {
            _marker: std::marker::PhantomData,
            ldc: LengthDelimitedCodec::default(),
        }
    }
}

impl<Codec, Decodable> Decoder for TcpMessageCodec<Codec>
where
    Codec: Decoder<Item = Decodable> + Send,
    Decodable: BorshDeserialize + Clone,
{
    type Item = TcpMessage<Decodable>;
    type Error = anyhow::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if src.is_empty() {
            return Ok(None);
        }

        let Some(src_ldc) = self.ldc.decode(src)? else {
            return Ok(None);
        };

        let msg: TcpMessage<Decodable> =
            borsh::from_slice(&src_ldc[..]).context("Decode TcpServerMessage wrapper type")?;

        Ok(Some(msg))
    }
}

impl<Codec, Encodable> Encoder<TcpMessage<Encodable>> for TcpMessageCodec<Codec>
where
    Codec: Encoder<Encodable> + Send,
    Encodable: BorshSerialize + Clone,
{
    type Error = anyhow::Error;

    fn encode(
        &mut self,
        item: TcpMessage<Encodable>,
        dst: &mut BytesMut,
    ) -> Result<(), Self::Error> {
        let serialized = borsh::to_vec(&item).context("Encoding to vec")?;

        self.ldc.encode(serialized.into(), dst)?;

        Ok(())
    }
}

/// A socket we can send data to
#[derive(Debug)]
struct SocketStream<Codec, In, Out>
where
    In: Clone,
    Out: Clone + std::fmt::Debug,
    Codec: Decoder<Item = In> + Encoder<Out> + Default,
{
    /// Last timestamp we received a ping from the peer.
    last_ping: u64,
    /// Sender to stream data to the peer
    sender: SplitSink<Framed<TcpStream, TcpMessageCodec<Codec>>, TcpMessage<Out>>,
    /// Handle to abort the receiving side of the stream
    abort: JoinHandle<()>,
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
                $crate::tcp::tcp_server::TcpServer::<ServerCodec, $req, $res>::start(port, stringify!($name).to_string()).await
            }
            pub async fn connect<Id: std::fmt::Display, A: $crate::net::ToSocketAddrs + std::fmt::Display>(id: Id, addr: A) -> Result<Client> {
                $crate::tcp::tcp_client::TcpClient::<ClientCodec, $req, $res>::connect(id, addr).await
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
                tcp,
                request: $crate::tcp::P2PTcpMessage<$msg>,
                response: $crate::tcp::P2PTcpMessage<$msg>
            }

            type P2PServerType = $crate::tcp::p2p_server::P2PServer<codec_tcp::ServerCodec, $msg>;

            pub async fn start_server(

                crypto: hyle_crypto::BlstCrypto,
                node_id: String,
                node_hostname: String,
                node_da_port: u16,
                node_p2p_port: u16,

            ) -> anyhow::Result<P2PServerType> {
                let server = codec_tcp::start_server(node_p2p_port).await?;

                Ok(
                    $crate::tcp::p2p_server::P2PServer::new(
                        std::sync::Arc::new(crypto),
                        node_id,
                        node_hostname,
                        node_da_port,
                        node_p2p_port,
                        server
                    )
                )

            }
        }
        }
    };
}

pub use p2p_server_mod;
