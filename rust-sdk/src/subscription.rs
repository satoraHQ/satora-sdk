//! Reconnecting swap-status hints. Neither WebSocket nor HTTP status is proof
//! of funding or settlement: verify the corresponding contract before acting.
use crate::Client;
use crate::Error;
use crate::Result;
use crate::control::Cancellation;
use crate::types::SwapStatus;
use futures_util::SinkExt;
use futures_util::StreamExt;
use serde_json::Value;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use tokio::sync::Notify;
use tokio::sync::broadcast;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_tungstenite::MaybeTlsStream;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::connect_async_with_config;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use url::Url;

const MAX_ACTIVE: usize = 256;
const MAX_ARGS: usize = 64;
#[derive(Clone, Debug)]
pub struct SubscriptionOptions {
    /// Override for a separately hosted socket; defaults to the API's `/ws`.
    pub websocket_url: Option<Url>,
    pub event_capacity: usize,
    pub max_tracked: usize,
    pub reconnect_min: Duration,
    pub reconnect_max: Duration,
    pub request_timeout: Duration,
    pub reconcile_interval: Duration,
    pub heartbeat_interval: Duration,
    pub idle_timeout: Duration,
}
impl Default for SubscriptionOptions {
    fn default() -> Self {
        Self {
            websocket_url: None,
            event_capacity: 512,
            max_tracked: 4096,
            reconnect_min: Duration::from_secs(1),
            reconnect_max: Duration::from_secs(30),
            request_timeout: Duration::from_secs(15),
            reconcile_interval: Duration::from_secs(30),
            heartbeat_interval: Duration::from_secs(20),
            idle_timeout: Duration::from_secs(60),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StatusSource {
    WebSocket,
    Http,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StatusEvent {
    Connected {
        reconnected: bool,
    },
    Disconnected,
    Update {
        swap_id: String,
        status: SwapStatus,
        source: StatusSource,
    },
    /// IDs exceeding the server's 256-subscription limit still receive HTTP hints.
    Queued {
        count: usize,
    },
    Missing {
        swap_id: String,
    },
    Reconciled,
    /// The consumer fell behind. An HTTP reconcile is already scheduled.
    Lagged {
        missed: u64,
    },
    Error {
        message: String,
    },
}

/// Owns its workers. Drop cancels immediately; `shutdown` waits for termination.
/// `subscribe`/`unsubscribe` coalesce changes rather than growing a command queue.
pub struct StatusSubscription {
    desired: watch::Sender<Vec<String>>,
    events: broadcast::Receiver<StatusEvent>,
    cancellation: Cancellation,
    reconcile: Arc<Notify>,
    task: Option<JoinHandle<()>>,
    max_tracked: usize,
}
impl StatusSubscription {
    pub fn subscribe(&self, ids: impl IntoIterator<Item = String>) -> Result<()> {
        let ids: Vec<_> = ids.into_iter().collect();
        if ids.iter().any(|id| uuid::Uuid::parse_str(id).is_err()) {
            return Err(Error::InvalidSwap(
                "status subscriptions require UUID swap IDs".into(),
            ));
        }
        let mut error = false;
        self.desired.send_if_modified(|current| {
            let mut next = current.clone();
            for id in ids {
                if !next.contains(&id) {
                    next.push(id);
                }
            }
            if next.len() > self.max_tracked {
                error = true;
                return false;
            }
            if next == *current {
                return false;
            }
            *current = next;
            true
        });
        if error {
            return Err(Error::InvalidSwap(
                "status subscription tracking limit exceeded".into(),
            ));
        }
        Ok(())
    }
    pub fn unsubscribe(&self, ids: &[String]) {
        self.desired.send_if_modified(|current| {
            let before = current.len();
            current.retain(|id| !ids.contains(id));
            before != current.len()
        });
    }
    pub fn reconcile(&self) {
        self.reconcile.notify_one();
    }
    pub async fn recv(&mut self) -> Option<StatusEvent> {
        match self.events.recv().await {
            Ok(event) => Some(event),
            Err(broadcast::error::RecvError::Lagged(missed)) => {
                self.reconcile();
                Some(StatusEvent::Lagged { missed })
            }
            Err(broadcast::error::RecvError::Closed) => None,
        }
    }
    pub async fn shutdown(mut self) -> Result<()> {
        self.cancellation.cancel();
        if let Some(task) = self.task.take() {
            task.await.map_err(|e| Error::Transport(e.to_string()))?;
        }
        Ok(())
    }
}
impl Drop for StatusSubscription {
    fn drop(&mut self) {
        self.cancellation.cancel();
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}
struct Emitter {
    events: broadcast::Sender<StatusEvent>,
    last: Mutex<HashMap<(String, StatusSource), SwapStatus>>,
}
impl Emitter {
    fn emit(&self, event: StatusEvent) {
        let _ = self.events.send(event);
    }
    fn update(&self, id: String, status: SwapStatus, source: StatusSource) {
        let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
        if last.get(&(id.clone(), source)) == Some(&status) {
            return;
        }
        last.insert((id.clone(), source), status.clone());
        drop(last);
        self.emit(StatusEvent::Update {
            swap_id: id,
            status,
            source,
        });
    }
    fn retain(&self, ids: &[String]) {
        self.last
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|(id, _), _| ids.contains(id));
    }
    fn error(&self, message: impl Into<String>) {
        self.emit(StatusEvent::Error {
            message: message.into(),
        });
    }
}
impl Client {
    pub fn subscribe_status(&self, options: SubscriptionOptions) -> Result<StatusSubscription> {
        if options.event_capacity == 0
            || options.event_capacity > 65536
            || options.max_tracked == 0
            || options.max_tracked > 65536
            || options.reconnect_min.is_zero()
            || options.reconnect_max < options.reconnect_min
            || options.request_timeout.is_zero()
            || options.reconcile_interval.is_zero()
            || options.heartbeat_interval.is_zero()
            || options.idle_timeout <= options.heartbeat_interval
        {
            return Err(Error::InvalidSwap("invalid subscription bounds".into()));
        }
        tokio::runtime::Handle::try_current().map_err(|_| {
            Error::InvalidSwap("status subscription requires a Tokio runtime".into())
        })?;
        let url = match &options.websocket_url {
            Some(url) => url.clone(),
            None => websocket_url(&self.base_url)?,
        };
        if !matches!(url.scheme(), "ws" | "wss") {
            return Err(Error::InvalidBaseUrl("expected ws or wss URL".into()));
        }
        let (desired, receiver) = watch::channel(Vec::new());
        let (events, event_receiver) = broadcast::channel(options.event_capacity);
        let emitter = Arc::new(Emitter {
            events,
            last: Mutex::new(HashMap::new()),
        });
        let cancellation = Cancellation::default();
        let stop = cancellation.clone();
        let reconcile = Arc::new(Notify::new());
        let notify = reconcile.clone();
        let client = self.clone();
        let max_tracked = options.max_tracked;
        let task = tokio::spawn(async move {
            tokio::select! {
                biased;
                _=stop.cancelled()=>{},
                _=async {
                    tokio::join!(socket_loop(url,receiver.clone(),&emitter,&notify,&options),http_loop(client,receiver,&emitter,&notify,&options));
                }=>{},
            }
        });
        Ok(StatusSubscription {
            desired,
            events: event_receiver,
            cancellation,
            reconcile,
            task: Some(task),
            max_tracked,
        })
    }
}
fn websocket_url(base: &Url) -> Result<Url> {
    let mut url = base.clone();
    let scheme = match url.scheme() {
        "https" => "wss",
        "http" => "ws",
        _ => return Err(Error::InvalidBaseUrl("expected HTTP API URL".into())),
    };
    url.set_scheme(scheme)
        .map_err(|_| Error::InvalidBaseUrl("invalid WebSocket scheme".into()))?;
    let path = format!("{}/ws", url.path().trim_end_matches('/'));
    url.set_path(&path);
    url.set_query(None);
    url.set_fragment(None);
    Ok(url)
}

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;
async fn frame(
    socket: &mut Socket,
    operation: &str,
    ids: &[String],
    timeout: Duration,
) -> Result<()> {
    for chunk in ids.chunks(MAX_ARGS) {
        let message = Message::Text(
            json!({"op":operation,"channel":"swap_status","args":chunk})
                .to_string()
                .into(),
        );
        tokio::time::timeout(timeout, socket.send(message))
            .await
            .map_err(|_| Error::Timeout("WebSocket send".into()))?
            .map_err(|e| Error::Transport(e.to_string()))?;
    }
    Ok(())
}
async fn sync_ids(
    socket: &mut Socket,
    current: &mut Vec<String>,
    desired: &[String],
    emitter: &Emitter,
    timeout: Duration,
) -> Result<()> {
    let next: Vec<_> = desired.iter().take(MAX_ACTIVE).cloned().collect();
    let removed: Vec<_> = current
        .iter()
        .filter(|id| !next.contains(id))
        .cloned()
        .collect();
    let added: Vec<_> = next
        .iter()
        .filter(|id| !current.contains(id))
        .cloned()
        .collect();
    frame(socket, "unsubscribe", &removed, timeout).await?;
    frame(socket, "subscribe", &added, timeout).await?;
    *current = next;
    emitter.retain(desired);
    emitter.emit(StatusEvent::Queued {
        count: desired.len().saturating_sub(MAX_ACTIVE),
    });
    Ok(())
}
fn process_message(text: &str, active: &[String], emitter: &Emitter) {
    let Ok(frame) = serde_json::from_str::<Value>(text) else {
        emitter.error("malformed WebSocket frame");
        return;
    };
    if frame["channel"] == "swap_status" {
        if let Some(data) = frame["data"].as_object() {
            for (id, status) in data {
                if !active.contains(id) {
                    continue;
                }
                if let Some(status) = status
                    .as_str()
                    .filter(|status| !status.is_empty() && status.len() <= 128)
                {
                    // Preserve regressions: status is a hint, and chain reorgs can
                    // legitimately move a swap backwards. Do not invent an ordering.
                    emitter.update(
                        id.clone(),
                        SwapStatus::from(status.to_string()),
                        StatusSource::WebSocket,
                    );
                } else {
                    emitter.error("invalid WebSocket status value");
                }
            }
        }
    } else if frame.get("message").is_some() {
        emitter.error("server rejected a WebSocket operation");
    }
}
async fn socket_loop(
    url: Url,
    mut desired: watch::Receiver<Vec<String>>,
    emitter: &Emitter,
    reconcile: &Notify,
    options: &SubscriptionOptions,
) {
    let mut backoff = options.reconnect_min;
    let mut reconnected = false;
    loop {
        let request = (|| {
            let mut request = url
                .as_str()
                .into_client_request()
                .map_err(|e| Error::Transport(e.to_string()))?;
            request.headers_mut().insert(
                "x-lendaswap-client",
                concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"))
                    .parse()
                    .expect("package version is a header"),
            );
            request.headers_mut().insert(
                "x-satora-server-version",
                env!("SATORA_SERVER_VERSION")
                    .parse()
                    .expect("server version is a header"),
            );
            Ok::<_, Error>(request)
        })();
        let Ok(request) = request else {
            emitter.error("invalid WebSocket request");
            return;
        };
        let config = WebSocketConfig::default()
            .max_message_size(Some(1024 * 1024))
            .max_frame_size(Some(1024 * 1024));
        match tokio::time::timeout(
            options.request_timeout,
            connect_async_with_config(request, Some(config), false),
        )
        .await
        {
            Ok(Ok((mut socket, _))) => {
                backoff = options.reconnect_min;
                emitter.emit(StatusEvent::Connected { reconnected });
                reconcile.notify_one();
                let mut active = Vec::new();
                let ids = desired.borrow_and_update().clone();
                if sync_ids(
                    &mut socket,
                    &mut active,
                    &ids,
                    emitter,
                    options.request_timeout,
                )
                .await
                .is_ok()
                {
                    let mut heartbeat = tokio::time::interval(options.heartbeat_interval);
                    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                    let mut received = Instant::now();
                    loop {
                        tokio::select! {
                            changed=desired.changed()=>{
                                if changed.is_err() {return;}
                                let ids=desired.borrow_and_update().clone();
                                if sync_ids(&mut socket,&mut active,&ids,emitter,options.request_timeout).await.is_err() {break;}
                            },
                            message=socket.next()=>match message {
                                Some(Ok(Message::Text(text)))=>{received=Instant::now();process_message(&text,&active,emitter);},
                                Some(Ok(Message::Ping(_)|Message::Pong(_)))=>{received=Instant::now();},
                                Some(Ok(Message::Close(_)))|Some(Err(_))|None=>break,
                                _=>{},
                            },
                            _=heartbeat.tick()=>{
                                if received.elapsed()>=options.idle_timeout {break;}
                                if !matches!(tokio::time::timeout(options.request_timeout,socket.send(Message::Ping(Vec::new().into()))).await,Ok(Ok(()))) {break;}
                            },
                        }
                    }
                }
            }
            Ok(Err(error)) => emitter.error(format!("WebSocket connection: {error}")),
            Err(_) => emitter.error("WebSocket connection deadline exceeded"),
        }
        emitter.emit(StatusEvent::Disconnected);
        reconnected = true;
        tokio::time::sleep(backoff).await;
        backoff = backoff.saturating_mul(2).min(options.reconnect_max);
    }
}
async fn http_loop(
    client: Client,
    mut desired: watch::Receiver<Vec<String>>,
    emitter: &Emitter,
    reconcile: &Notify,
    options: &SubscriptionOptions,
) {
    let mut interval = tokio::time::interval(options.reconcile_interval);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _=interval.tick()=>{},
            _=reconcile.notified()=>{},
            result=desired.changed()=>{if result.is_err() {return;}},
        }
        let ids = desired.borrow_and_update().clone();
        emitter.retain(&ids);
        let result = tokio::time::timeout(options.request_timeout, async {
            for chunk in ids.chunks(100) {
                let response = client.get_bulk_status(chunk).await?;
                // Subscription changes can happen while HTTP is in flight.
                let current = desired.borrow().clone();
                for entry in response.statuses {
                    if chunk.contains(&entry.id) && current.contains(&entry.id) {
                        emitter.update(entry.id, entry.status, StatusSource::Http);
                    }
                }
                for id in response.not_found {
                    if chunk.contains(&id) && current.contains(&id) {
                        emitter.emit(StatusEvent::Missing { swap_id: id });
                    }
                }
            }
            Ok::<_, Error>(())
        })
        .await;
        match result {
            Ok(Ok(())) => emitter.emit(StatusEvent::Reconciled),
            Ok(Err(error)) => emitter.error(error.to_string()),
            Err(_) => emitter.error("HTTP status reconciliation deadline exceeded"),
        }
    }
}

#[cfg(test)]
mod tests;
