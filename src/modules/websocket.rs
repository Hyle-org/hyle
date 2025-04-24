use std::sync::Arc;
use std::time::Duration;

use crate::{
    bus::{BusClientSender, BusMessage, SharedMessageBus},
    module_bus_client, module_handle_messages,
    utils::modules::Module,
};
use anyhow::{anyhow, Context, Error, Result};
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Router,
};
use futures::{
    sink::SinkExt,
    stream::{SplitSink, SplitStream, StreamExt},
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use tokio::{sync::Mutex, task::JoinSet};
use tracing::{debug, error, info};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsInMessage<T> {
    pub message: T,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsOutMessage<T> {
    pub message: T,
}
impl<T> WsOutMessage<T> {
    pub fn new(message: T) -> Self {
        Self { message }
    }
}

module_bus_client! {
#[derive(Debug)]
pub struct WebSocketBusClient<In: Send + Sync + Clone + 'static, Out: Send + Sync + Clone + 'static> {
    sender(WsInMessage<In>),
    receiver(WsOutMessage<Out>),
}
}

impl<I> BusMessage for WsInMessage<I> {}
impl<I> BusMessage for WsOutMessage<I> {}

/// Configuration for the WebSocket module
#[derive(Debug, Clone)]
pub struct WebSocketConfig {
    /// The port number to bind the WebSocket server to
    pub port: u16,
    /// The endpoint path for WebSocket connections
    pub ws_path: String,
    /// The endpoint path for health checks
    pub health_path: String,
    /// The interval at which to check for new peers
    pub peer_check_interval: Duration,
}

impl Default for WebSocketConfig {
    fn default() -> Self {
        Self {
            port: 8080,
            ws_path: "/ws".to_string(),
            health_path: "/ws_health".to_string(),
            peer_check_interval: Duration::from_millis(100),
        }
    }
}

/// A WebSocket module that sends messages from the bus to WebSocket clients
/// and vice versa
pub struct WebSocketModule<In, Out>
where
    In: Send + Sync + Clone + 'static,
    Out: Serialize + Send + Sync + Clone + 'static,
{
    bus: WebSocketBusClient<In, Out>,
    app: Option<Router>,
    peer_senders: Vec<SplitSink<WebSocket, Message>>,
    #[allow(clippy::type_complexity)]
    peer_receivers: JoinSet<Option<(SplitStream<WebSocket>, Result<WsInMessage<In>, Error>)>>,
    new_peers: NewPeers,
    config: WebSocketConfig,
}

#[derive(Clone, Default)]
struct NewPeers(pub Arc<Mutex<Vec<WebSocket>>>);

pub struct WebSocketModuleCtx {
    pub bus: SharedMessageBus,
    pub config: WebSocketConfig,
}

impl<In, Out> Module for WebSocketModule<In, Out>
where
    In: DeserializeOwned + std::fmt::Debug + Send + Sync + Clone + 'static,
    Out: Serialize + Send + Sync + Clone + 'static,
{
    type Context = WebSocketModuleCtx;

    async fn build(ctx: Self::Context) -> Result<Self> {
        let config = ctx.config;
        let new_peers = NewPeers::default();
        let app = Router::new()
            .route(&config.ws_path, get(ws_handler))
            .route(&config.health_path, get(health_check))
            .with_state(new_peers.clone());

        Ok(Self {
            bus: WebSocketBusClient::new_from_bus(ctx.bus.new_handle()).await,
            app: Some(app),
            peer_senders: Vec::new(),
            peer_receivers: JoinSet::new(),
            new_peers,
            config,
        })
    }

    async fn run(&mut self) -> Result<()> {
        // Start the server
        let bind_addr = format!("0.0.0.0:{}", self.config.port);
        let listener = tokio::net::TcpListener::bind(&bind_addr)
            .await
            .map_err(|e| anyhow!("Failed to bind to port {}: {}", self.config.port, e))?;

        let app = self
            .app
            .take()
            .ok_or_else(|| anyhow!("Router was already taken"))?;

        info!("WebSocket server listening on port {}", self.config.port);

        let server = tokio::spawn(async move {
            if let Err(e) = axum::serve(listener, app).await {
                error!("Server error: {}", e);
            } else {
                info!("Server stopped");
            }
        });

        module_handle_messages! {
            on_bus self.bus,
            listen<WsOutMessage<Out>> msg => {
                if let Err(e) = self.handle_outgoing_message(msg.message).await {
                    error!("Error sending outbound message: {}", e);
                    break;
                }
            }
            Some(Ok(Some(msg))) = self.peer_receivers.join_next() => {
                match msg {
                    (socket_stream, Ok(msg)) => {
                        debug!("Received message: {:?}", msg);
                        if let Err(e) = self.handle_incoming_message(msg).await {
                            error!("Error handling incoming message: {}", e);
                            break;
                        }
                        // Add it again to the receiver
                        self.peer_receivers.spawn(Self::process_websocket_incoming(socket_stream));
                    }
                    (_, Err(e)) => {
                        error!("Error receiving message: {}", e);
                        break;
                    }
                }
            }
            _ = tokio::time::sleep(self.config.peer_check_interval) => {
                // Check for new peers
                let mut peers = self.new_peers.0.lock().await;
                for peer in peers.drain(..) {
                    let (sender, receiver) = peer.split();
                    self.peer_senders.push(sender);
                    self.peer_receivers.spawn(Self::process_websocket_incoming(receiver));
                }
            }
        };

        server.abort_handle().abort();
        Ok(())
    }
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<NewPeers>) -> impl IntoResponse {
    ws.on_upgrade(async move |socket| {
        debug!("New WebSocket connection established");
        let mut state = state.0.lock().await;
        state.push(socket);
    })
}

impl<In, Out> WebSocketModule<In, Out>
where
    In: DeserializeOwned + std::fmt::Debug + Send + Sync + Clone + 'static,
    Out: Serialize + Send + Sync + Clone + 'static,
{
    async fn handle_incoming_message(&mut self, msg: WsInMessage<In>) -> Result<()> {
        self.bus
            .send(msg)
            .context("Failed to send inbound message")?;
        Ok(())
    }

    async fn handle_outgoing_message(&mut self, msg: Out) -> Result<()> {
        let mut at_least_one_ok = false;

        let text = serde_json::to_string(&msg).context("Failed to serialize outbound message")?;
        let text: Message = Message::Text(text.clone().into());

        let send_futures: Vec<_> = self
            .peer_senders
            .iter_mut()
            .map(|peer| {
                let text = text.clone();
                async move { peer.send(text).await }
            })
            .collect();

        let results = futures::future::join_all(send_futures).await;

        for idx in (0..self.peer_senders.len()).rev() {
            match &results.get(idx) {
                Some(Ok(_)) => {
                    at_least_one_ok = true;
                }
                Some(Err(e)) => {
                    debug!("Failed to send message to WebSocket: {}", e);
                    let _ = self.peer_senders.swap_remove(idx);
                }
                None => {
                    debug!("No result for WebSocket sender at index {}", idx);
                }
            }
        }

        if at_least_one_ok || self.peer_senders.is_empty() {
            Ok(())
        } else {
            Err(anyhow!("Failed to send message to all WebSocket peers"))
        }
    }

    async fn process_websocket_incoming(
        mut receiver: SplitStream<WebSocket>,
    ) -> Option<(SplitStream<WebSocket>, Result<WsInMessage<In>, Error>)> {
        while let Some(msg) = receiver.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    debug!("Received message: {:?}", text);
                    return Some((
                        receiver,
                        serde_json::from_str::<In>(text.as_str())
                            .context("Failed to parse message")
                            .map(|message| WsInMessage { message }),
                    ));
                }
                Ok(Message::Close(_)) => {
                    debug!("Client initiated close");
                    break;
                }
                Err(e) => {
                    error!("WebSocket receive error: {}", e);
                    break;
                }
                _ => {} // Ignore other message types
            }
        }
        None
    }
}

async fn health_check() -> impl IntoResponse {
    (StatusCode::OK, "OK")
}
