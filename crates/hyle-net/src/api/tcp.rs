use borsh::{BorshDeserialize, BorshSerialize};
use sdk::Transaction;

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize, Eq, PartialEq)]
pub enum TcpServerMessage {
    NewTx(Transaction),
}
#[derive(Debug, Clone, BorshSerialize, BorshDeserialize, Eq, PartialEq)]
pub struct TcpServerResponse;

crate::tcp_client_server! {
    pub TcpServer,
    request: TcpServerMessage,
    response: TcpServerResponse
}
