use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use serde::de::{DeserializeOwned, Deserializer};
use serde::{Deserialize, Serialize};
use std::error::Error;
use std::fmt;
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio::time::{self, Duration, Instant};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use tokio_tungstenite::{tungstenite, MaybeTlsStream, WebSocketStream};

/// Default Pusher activity timeout (seconds) assumed until the server's
/// `pusher:connection_established` event reports the real value.
const DEFAULT_ACTIVITY_TIMEOUT: Duration = Duration::from_secs(120);
/// Default number of consecutive reconnect attempts before giving up.
///
/// With exponential backoff this spans roughly two minutes, which rides out
/// ordinary network blips and Kick-side restarts. Long-running consumers should
/// still supervise the client and rebuild it if this is exhausted - see
/// [`KickClient::set_max_reconnect_attempts`].
const DEFAULT_MAX_RECONNECT_ATTEMPTS: u32 = 8;

/// Ceiling on the exponential reconnect delay.
const MAX_RECONNECT_DELAY: Duration = Duration::from_secs(30);

/// A WebSocket client for connecting to and reading messages from Kick chatroom.
/// Write half of the socket, shared with [`KickSubscriptions`].
type SharedSink = Arc<Mutex<SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>>>;

/// A handle for changing subscriptions on a live [`KickClient`].
///
/// Separate from the client because the read loop owns `&mut KickClient` for
/// as long as it is running; without this, adding a chatroom would have to
/// wait for the next inbound message, which on a quiet chat can be minutes.
#[derive(Clone)]
pub struct KickSubscriptions {
    write: SharedSink,
    channel_ids: Arc<Mutex<Vec<u64>>>,
}

impl KickSubscriptions {
    /// Subscribes to another chatroom on the existing connection.
    ///
    /// Idempotent: subscribing to a chatroom already in the list is a no-op,
    /// so a caller polling for changes cannot accumulate duplicates.
    pub async fn subscribe(&self, chatroom_id: u64) -> Result<(), KickError> {
        {
            let mut ids = self.channel_ids.lock().await;
            if ids.contains(&chatroom_id) {
                return Ok(());
            }
            ids.push(chatroom_id);
        }

        let message = serde_json::json!({
            "event": "pusher:subscribe",
            "data": { "auth": "", "channel": format!("chatrooms.{}.v2", chatroom_id) }
        });

        self.write
            .lock()
            .await
            .send(Message::Text(message.to_string().into()))
            .await?;

        Ok(())
    }

    /// Unsubscribes from a chatroom on the existing connection.
    pub async fn unsubscribe(&self, chatroom_id: u64) -> Result<(), KickError> {
        {
            let mut ids = self.channel_ids.lock().await;
            if !ids.iter().any(|id| *id == chatroom_id) {
                return Ok(());
            }
            ids.retain(|id| *id != chatroom_id);
        }

        let message = serde_json::json!({
            "event": "pusher:unsubscribe",
            "data": { "channel": format!("chatrooms.{}.v2", chatroom_id) }
        });

        self.write
            .lock()
            .await
            .send(Message::Text(message.to_string().into()))
            .await?;

        Ok(())
    }

    /// The chatrooms currently subscribed to.
    pub async fn subscribed(&self) -> Vec<u64> {
        self.channel_ids.lock().await.clone()
    }
}

pub struct KickClient {
    /// The WebSocket URL used to connect to the Kick server.
    url: String,
    /// The chatrooms this client is subscribed to.
    ///
    /// Shared and mutable so subscriptions added at runtime are also restored
    /// after a reconnect - Pusher subscriptions do not survive a dropped
    /// socket, so a list captured at construction would silently lose every
    /// channel added later.
    channel_ids: Arc<Mutex<Vec<u64>>>,
    /// The WebSocket write sink, used for keep-alive pings/pongs and for
    /// subscribing to further chatrooms while the connection is live.
    write: SharedSink,
    /// The WebSocket read stream for receiving messages.
    read_stream: SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>,
    /// How long the connection can stay idle before it's considered stale.
    /// Learned from the server's `pusher:connection_established` event.
    activity_timeout: Duration,
    /// When the last frame was received (or a keep-alive ping was sent).
    last_activity: Instant,
    /// How many consecutive reconnect attempts to make before returning an
    /// error to the caller. `None` retries indefinitely.
    max_reconnect_attempts: Option<u32>,
}

impl KickClient {
    /// Creates a new instance of `KickClient` and automatically establishes a WebSocket connection.
    ///
    /// # Arguments
    ///
    /// * `url` - The WebSocket URL to connect to.
    /// * `channel_ids` - The IDs of the chatrooms to subscribe to.
    ///
    /// # Returns
    ///
    /// A `KickClient` instance ready to receive messages.
    ///
    /// # Errors
    ///
    /// This function will return an error if the WebSocket connection fails.
    /// # Examples
    ///
    /// ```no_run
    /// use kick_client::KickClient;
    ///
    /// # async fn run() {
    /// let mut client = KickClient::new("wss://ws-us2.pusher.com/app/32cbd69e4b950bf97679?protocol=7&client=js&version=8.4.0-rc2&flash=false", vec![281473]).await.unwrap();
    /// while let Some(message) = client.read_message().await.unwrap() {
    ///     println!("{:?}", message);
    /// }
    /// # }
    /// ```
    pub async fn new(url: &str, channel_ids: Vec<u64>) -> Result<Self, Box<dyn Error>> {
        let (write, read_stream) = Self::connect(url, &channel_ids).await?;

        Ok(Self {
            url: url.to_string(),
            channel_ids: Arc::new(Mutex::new(channel_ids)),
            write: Arc::new(Mutex::new(write)),
            read_stream,
            activity_timeout: DEFAULT_ACTIVITY_TIMEOUT,
            last_activity: Instant::now(),
            max_reconnect_attempts: Some(DEFAULT_MAX_RECONNECT_ATTEMPTS),
        })
    }

    /// Sets how many consecutive reconnect attempts to make before
    /// [`KickClient::read_message`] returns an error.
    ///
    /// `None` retries forever, which suits a process whose only job is to stay
    /// connected. Prefer a bounded count when the caller supervises the client
    /// itself, so that a prolonged outage becomes visible instead of being
    /// absorbed silently inside `read_message`.
    pub fn set_max_reconnect_attempts(&mut self, attempts: Option<u32>) {
        self.max_reconnect_attempts = attempts;
    }

    /// A handle for changing subscriptions while the read loop is running.
    pub fn subscriptions(&self) -> KickSubscriptions {
        KickSubscriptions {
            write: self.write.clone(),
            channel_ids: self.channel_ids.clone(),
        }
    }

    /// Resolves a channel's slug (e.g. `"xyamii"`) to the numeric chatroom ID
    /// that [`KickClient::new`] expects in `channel_ids`.
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails or the channel doesn't exist.
    pub async fn resolve_chatroom_id(channel_slug: &str) -> Result<u64, KickError> {
        #[derive(Deserialize)]
        struct ChannelLookupResponse {
            chatroom: ChatroomLookup,
        }

        #[derive(Deserialize)]
        struct ChatroomLookup {
            id: u64,
        }

        let url = format!("https://kick.com/api/v2/channels/{}", channel_slug);
        let response = reqwest::Client::new()
            .get(&url)
            .header(
                "User-Agent",
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/128.0 Safari/537.36",
            )
            .send()
            .await?
            .error_for_status()?
            .json::<ChannelLookupResponse>()
            .await?;

        Ok(response.chatroom.id)
    }

    /// Opens a new WebSocket connection to `url` and subscribes to each of
    /// `channel_ids`, returning the split sink/stream halves.
    async fn connect(
        url: &str,
        channel_ids: &[u64],
    ) -> Result<
        (
            SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>,
            SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>,
        ),
        KickError,
    > {
        let request = url.into_client_request()?;
        let (ws_stream, _) = connect_async(request).await?;
        let (mut write, read) = ws_stream.split();

        for channel_id in channel_ids {
            let subscribe_message = serde_json::json!({
                "event": "pusher:subscribe",
                "data": {
                    "auth": "",
                    "channel": format!("chatrooms.{}.v2", channel_id)
                }
            });

            write
                .send(Message::Text(subscribe_message.to_string().into()))
                .await?;
        }

        Ok((write, read))
    }

    /// Re-establishes the connection and re-subscribes to the original
    /// channels, with a short backoff between attempts.
    async fn reconnect(&mut self) -> Result<(), KickError> {
        let mut attempt: u32 = 0;
        loop {
            attempt += 1;
            // Snapshot rather than hold the lock across the connect: the list
            // includes anything subscribed at runtime, which must be restored
            // or those channels go silently unlogged after a blip.
            let channel_ids = self.channel_ids.lock().await.clone();

            match Self::connect(&self.url, &channel_ids).await {
                Ok((write, read_stream)) => {
                    *self.write.lock().await = write;
                    self.read_stream = read_stream;
                    self.activity_timeout = DEFAULT_ACTIVITY_TIMEOUT;
                    self.last_activity = Instant::now();
                    return Ok(());
                }
                Err(err) => {
                    if self
                        .max_reconnect_attempts
                        .is_some_and(|max| attempt >= max)
                    {
                        return Err(err);
                    }

                    time::sleep(Self::reconnect_delay(attempt)).await;
                }
            }
        }
    }

    /// Delay before reconnect attempt `attempt` (1-based).
    ///
    /// Exponential rather than the previous linear ramp: linear growth spent
    /// its whole budget inside the first few seconds of an outage, so anything
    /// longer than a momentary blip exhausted the retries.
    fn reconnect_delay(attempt: u32) -> Duration {
        let exponent = attempt.saturating_sub(1).min(6);
        Duration::from_secs(1)
            .saturating_mul(1 << exponent)
            .min(MAX_RECONNECT_DELAY)
    }

    /// Replies to a server-initiated `pusher:ping` so the connection isn't
    /// dropped as inactive.
    async fn send_pong(&mut self) -> Result<(), KickError> {
        self.write
            .lock()
            .await
            .send(Message::Text(
                serde_json::json!({"event": "pusher:pong", "data": {}})
                    .to_string()
                    .into(),
            ))
            .await?;
        Ok(())
    }

    /// Proactively pings the server after an idle period, in case a server
    /// ping was missed (e.g. behind a proxy that silently drops the socket).
    async fn send_ping(&mut self) -> Result<(), KickError> {
        self.write
            .lock()
            .await
            .send(Message::Text(
                serde_json::json!({"event": "pusher:ping", "data": {}})
                    .to_string()
                    .into(),
            ))
            .await?;
        Ok(())
    }

    /// Reads the next message from the WebSocket stream and returns a parsed `KickChatMessage`.
    ///
    /// Internally responds to server keep-alive pings, sends its own idle
    /// keep-alive pings, and transparently reconnects (re-subscribing to the
    /// original channels) if the connection drops — none of that is surfaced
    /// to the caller.
    ///
    /// # Errors
    ///
    /// Returns an error if the WebSocket stream encounters an error that
    /// reconnecting can't recover from.
    pub async fn read_message(&mut self) -> Result<Option<KickChatMessage>, KickError> {
        loop {
            let wait = self.activity_timeout.saturating_sub(self.last_activity.elapsed());

            match time::timeout(wait, self.read_stream.next()).await {
                Ok(Some(Ok(Message::Text(text)))) => {
                    self.last_activity = Instant::now();

                    match serde_json::from_str::<KickChatMessage>(&text) {
                        Ok(parsed_message) => match &parsed_message.data {
                            MessageData::PusherPing(_) => {
                                // Same reasoning as the idle ping above.
                                if self.send_pong().await.is_err() {
                                    self.reconnect().await?;
                                }
                                continue;
                            }
                            MessageData::PusherConnectionEstablished(data) => {
                                self.activity_timeout =
                                    Duration::from_secs(data.activity_timeout as u64);
                                return Ok(Some(parsed_message));
                            }
                            _ => return Ok(Some(parsed_message)),
                        },
                        Err(e) => {
                            return Ok(Some(KickChatMessage {
                                data: MessageData::Unsupported(Some(text.to_string()), e.to_string()),
                                channel: None,
                            }))
                        }
                    }
                }
                // Non-text frame (ping/pong/binary/close) - nothing for the caller to see.
                Ok(Some(Ok(_))) => {
                    self.last_activity = Instant::now();
                }
                // Connection-level error or the stream ended: try to recover transparently.
                Ok(Some(Err(_))) | Ok(None) => {
                    self.reconnect().await?;
                }
                // Idle for `activity_timeout` with no traffic - nudge the server.
                // A failed write means the socket is half-open, which is the
                // usual way a long-lived connection dies; reconnect rather than
                // handing the caller an error it can only respond to by
                // rebuilding the client anyway.
                Err(_) => {
                    if self.send_ping().await.is_err() {
                        self.reconnect().await?;
                        continue;
                    }
                    self.last_activity = Instant::now();
                }
            }
        }
    }

    /// If the `tokio-handling` feature is enabled, this function spawns a task that handles
    /// incoming messages and invokes the provided callback for each message.
    #[cfg(feature = "tokio-handling")]
    pub fn start_handling<F>(mut self, callback: F)
    where
        F: Fn(KickChatMessage) + Send + Sync + 'static,
    {
        tokio::spawn(async move {
            while let Ok(Some(message)) = self.read_message().await {
                callback(message);
            }
        });
    }
}

/// Enum representing different types of messages received from the WebSocket.
#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "event", content = "data")]
pub enum MessageData {
    /// A chat message received in the chatroom.
    #[serde(rename = "App\\Events\\ChatMessageEvent")]
    #[serde(deserialize_with = "json_string_to_struct")]
    ChatMessage(ChatMessageEventData),
    /// A message indicating that user's message was deleted.
    #[serde(rename = "App\\Events\\MessageDeletedEvent")]
    #[serde(deserialize_with = "json_string_to_struct")]
    DeletedMessage(DeletedMessageEventData),
    /// A message indicating that a user was banned from the chatroom.
    #[serde(rename = "App\\Events\\UserBannedEvent")]
    #[serde(deserialize_with = "json_string_to_struct")]
    UserBanned(UserBannedEventData),
    /// A message indicating that a user was unbanned from the chatroom.
    #[serde(rename = "App\\Events\\UserUnbannedEvent")]
    #[serde(deserialize_with = "json_string_to_struct")]
    UserUnbanned(UserUnbannedEventData),
    /// A message indicating that the chatroom was updated.
    #[serde(rename = "App\\Events\\ChatroomUpdatedEvent")]
    #[serde(deserialize_with = "json_string_to_struct")]
    ChatroomUpdated(ChatroomUpdatedEventData),
    /// A message indicating that the chatroom was cleared.
    #[serde(rename = "App\\Events\\ChatroomClearEvent")]
    #[serde(deserialize_with = "json_string_to_struct")]
    ChatroomClear(ChatroomClearEventData),
    /// A message indicating that a poll was updated.
    #[serde(rename = "App\\Events\\PollUpdateEvent")]
    #[serde(deserialize_with = "json_string_to_struct")]
    PollUpdate(PollUpdateEventData),
    /// A message indicating that a poll was deleted.
    #[serde(rename = "App\\Events\\PollDeleteEvent")]
    #[serde(deserialize_with = "json_string_to_struct")]
    PollDelete(PollDeleteEventData),
    /// A message indicating that the connection was established.
    #[serde(rename = "pusher:connection_established")]
    #[serde(deserialize_with = "json_string_to_struct")]
    PusherConnectionEstablished(PusherConnectionEstablishedEventData),
    /// A message indicating that a subscription was pushed and was successful.
    #[serde(rename = "pusher_internal:subscription_succeeded")]
    #[serde(deserialize_with = "json_string_to_struct")]
    PusherSubscriptionSucceeded(PusherSubscriptionSucceededEventData),
    /// A messenge indicating that the connection is still alive.
    #[serde(rename = "pusher:pong")]
    #[serde(deserialize_with = "json_string_to_struct")]
    PusherPong(PusherPongEventData),
    /// A keep-alive request from the server; the client must reply with `pusher:pong`.
    /// Handled internally by [`KickClient::read_message`] and never returned to callers.
    #[serde(rename = "pusher:ping")]
    #[serde(deserialize_with = "json_string_to_struct")]
    PusherPing(PusherPingEventData),
    /// A messenge indicating that someone purchased subscription in the channel.
    #[serde(rename = "App\\Events\\SubscriptionEvent")]
    #[serde(deserialize_with = "json_string_to_struct")]
    SubscriptionEvent(SubscriptionEventData),
    /// A messenge indicating that someone's message was unpinned.
    #[serde(rename = "App\\Events\\PinnedMessageDeletedEvent")]
    #[serde(deserialize_with = "json_string_to_struct")]
    PinnedMessageDeletedEvent(PinnedMessageDeletedEventData),
    /// A messenge indicating that someone's message was pinned.
    #[serde(rename = "App\\Events\\PinnedMessageCreatedEvent")]
    #[serde(deserialize_with = "json_string_to_struct")]
    PinnedMessageCreatedEvent(PinnedMessageCreatedEventData),
    /// A message of unknown type.
    Unknown(Option<String>),
    /// A message of unsupported type yet. Feel free to submit it to me.
    Unsupported(Option<String>, String)
}

/// Data structure containing the content of a message.
#[derive(Serialize, Deserialize, Debug)]
pub struct KickChatMessage {
    #[serde(flatten)]
    pub data: MessageData,
    /// Channel id.
    pub channel: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ChatMessageEventData {
    pub id: String,
    pub chatroom_id: u32,
    pub content: Option<String>,
    /// `"message"` for an ordinary message, `"reply"` when replying to another.
    pub r#type: Option<String>,
    pub created_at: Option<String>,
    pub sender: ChatMessageSender,
    /// Present when `r#type` is `"reply"`; identifies the message replied to.
    #[serde(default)]
    pub metadata: Option<ChatMessageMetadata>,
}

/// Extra data attached to a message. Currently only reply information.
#[derive(Serialize, Deserialize, Debug)]
pub struct ChatMessageMetadata {
    #[serde(default)]
    pub original_sender: Option<RepliedSender>,
    #[serde(default)]
    pub original_message: Option<RepliedMessage>,
}

/// The author of the message being replied to.
#[derive(Serialize, Deserialize, Debug)]
pub struct RepliedSender {
    /// Kick has sent this as both a JSON number and a string across API
    /// revisions, so it is normalised to a string rather than pinned to one.
    #[serde(default, deserialize_with = "flexible_id")]
    pub id: Option<String>,
    #[serde(default)]
    pub username: Option<String>,
}

/// The message being replied to.
#[derive(Serialize, Deserialize, Debug)]
pub struct RepliedMessage {
    #[serde(default, deserialize_with = "flexible_id")]
    pub id: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
}

/// Accepts a JSON string, number, or null and yields `Option<String>`.
fn flexible_id<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    match serde_json::Value::deserialize(deserializer)? {
        serde_json::Value::Null => Ok(None),
        serde_json::Value::String(s) => Ok(Some(s)),
        serde_json::Value::Number(n) => Ok(Some(n.to_string())),
        other => Ok(Some(other.to_string())),
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ChatMessageSender {
    pub id: u32,
    pub username: String,
    pub slug: Option<String>,
    pub identity: ChatMessageSenderIdentity,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ChatMessageSenderIdentity {
    pub color: Option<String>,
    pub badges: Vec<ChatMessageSenderBadge>,
}

#[derive(Serialize, Debug)]
pub enum ChatMessageSenderBadge {
    FullBadge {
        r#type: String,
        text: String,
        count: Option<u32>,
    },
    SimpleBadge {
        r#type: String,
        text: String,
    },
}

#[derive(Serialize, Deserialize, Debug)]
pub struct UserBannedEventData {
    pub id: String,
    pub user: User,
    pub banned_by: User,
    pub permanent: bool,
    pub duration: Option<u64>,
    pub expires_at: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct UserUnbannedEventData {
    pub id: String,
    pub user: User,
    pub unbanned_by: User,
    pub permanent: bool,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct User {
    pub id: u32,
    pub username: String,
    pub slug: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ChatroomUpdatedEventData {
    pub id: u32,
    pub slow_mode: SlowMode,
    pub subscribers_mode: SubscribersMode,
    pub followers_mode: FollowersMode,
    pub emotes_mode: EmotesMode,
    pub advanced_bot_protection: AdvancedBotProtection,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct SlowMode {
    pub enabled: bool,
    pub message_interval: u64,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct SubscribersMode {
    pub enabled: bool,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct FollowersMode {
    pub enabled: bool,
    pub min_duration: u64,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct EmotesMode {
    pub enabled: bool,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct AdvancedBotProtection {
    pub enabled: bool,
    pub remaining_time: u64,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ChatroomClearEventData {
    pub id: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct DeletedMessageEventData {
    pub id: String,
    pub message: DeletedMessage,
    #[serde(rename = "aiModerated")]
    pub ai_moderated: bool,
    #[serde(rename = "violatedRules")]
    pub violated_rules: Option<Vec<String>>
}

#[derive(Serialize, Deserialize, Debug)]
pub struct DeletedMessage {
    pub id: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct PollUpdateEventData {
    pub poll: Poll,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Poll {
    pub title: String,
    pub options: Vec<PollOption>,
    pub duration: u32,
    pub remaining: u32,
    pub result_display_duration: u32,
    pub has_voted: Option<bool>,
    pub voted_option_id: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct PollOption {
    pub id: u32,
    pub label: String,
    pub votes: u32,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct PollDeleteEventData {}

#[derive(Serialize, Deserialize, Debug)]
pub struct PusherConnectionEstablishedEventData {
    pub socket_id: String,
    pub activity_timeout: u32,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct PusherSubscriptionSucceededEventData {}

#[derive(Serialize, Deserialize, Debug)]
pub struct SubscriptionEventData {
    pub chatroom_id: u32,
    pub username: String,
    pub months: u32
}

#[derive(Serialize, Deserialize, Debug)]
pub struct StreamHostEventData {
    pub chatroom_id: u32,
    pub optional_message: Option<String>,
    pub number_viewers: u32,
    pub host_username: String
}

#[derive(Serialize, Deserialize, Debug)]
pub struct PinnedMessageDeletedEventData {}

#[derive(Serialize, Deserialize, Debug)]
pub struct PinnedMessageCreatedEventData {
    pub message: ChatMessageEventData,
    pub duration: String,
    #[serde(rename = "pinnedBy")]
    pub pinned_by: ChatMessageSender,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct PusherPongEventData {}

#[derive(Serialize, Deserialize, Debug)]
pub struct PusherPingEventData {}

impl<'de> Deserialize<'de> for ChatMessageSenderBadge {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize, Debug)]
        struct BadgeHelper {
            r#type: String,
            text: String,
            count: Option<u32>,
        }

        let helper = BadgeHelper::deserialize(deserializer)?;
        if let Some(count) = helper.count {
            Ok(ChatMessageSenderBadge::FullBadge {
                r#type: helper.r#type,
                text: helper.text,
                count: Some(count),
            })
        } else {
            Ok(ChatMessageSenderBadge::SimpleBadge {
                r#type: helper.r#type,
                text: helper.text,
            })
        }
    }
}
fn json_string_to_struct<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    let s = String::deserialize(deserializer)?;
    serde_json::from_str(&s).map_err(|e| {
        serde::de::Error::custom(e)
    })}

/// Enum representing possible errors in KickClient.
#[derive(Debug)]
pub enum KickError {
    WebSocketError(tungstenite::Error),
    MessageParseError(serde_json::Error),
    HttpError(reqwest::Error),
    StreamEnded,
}

impl fmt::Display for KickError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KickError::WebSocketError(err) => write!(f, "WebSocket error: {}", err),
            KickError::MessageParseError(err) => write!(f, "Message parse error: {}", err),
            KickError::HttpError(err) => write!(f, "HTTP error: {}", err),
            KickError::StreamEnded => write!(f, "WebSocket stream ended unexpectedly"),
        }
    }
}

impl std::error::Error for KickError {}

impl From<tungstenite::Error> for KickError {
    fn from(err: tungstenite::Error) -> Self {
        KickError::WebSocketError(err)
    }
}

impl From<serde_json::Error> for KickError {
    fn from(err: serde_json::Error) -> Self {
        KickError::MessageParseError(err)
    }
}

impl From<reqwest::Error> for KickError {
    fn from(err: reqwest::Error) -> Self {
        KickError::HttpError(err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::net::TcpListener;
    use tokio_tungstenite::accept_async;

    fn established_frame() -> String {
        serde_json::json!({
            "event": "pusher:connection_established",
            "data": serde_json::json!({"socket_id": "123.456", "activity_timeout": 120}).to_string()
        })
        .to_string()
    }

    fn subscribed_frame() -> String {
        serde_json::json!({
            "event": "pusher_internal:subscription_succeeded",
            "data": "{}",
            "channel": "chatrooms.4598.v2"
        })
        .to_string()
    }

    fn ping_frame() -> String {
        serde_json::json!({"event": "pusher:ping", "data": "{}"}).to_string()
    }

    fn chat_message_frame(badges: serde_json::Value) -> String {
        let inner = serde_json::json!({
            "id": "11111111-1111-1111-1111-111111111111",
            "chatroom_id": 4598,
            "content": "hello",
            "type": "message",
            "created_at": "2026-01-01T00:00:00+00:00",
            "sender": {
                "id": 1,
                "username": "tester",
                "slug": "tester",
                "identity": {"color": "#FFFFFF", "badges": badges}
            }
        })
        .to_string();

        serde_json::json!({
            "event": "App\\Events\\ChatMessageEvent",
            "data": inner,
            "channel": "chatrooms.4598.v2"
        })
        .to_string()
    }

    // ---- Pure deserialization tests ----

    #[test]
    fn parses_chat_message_with_full_badge() {
        let frame = chat_message_frame(serde_json::json!([
            {"type": "subscriber", "text": "Subscriber", "count": 5}
        ]));
        let parsed: KickChatMessage = serde_json::from_str(&frame).unwrap();
        match parsed.data {
            MessageData::ChatMessage(m) => {
                assert_eq!(m.sender.username, "tester");
                assert!(matches!(
                    m.sender.identity.badges.as_slice(),
                    [ChatMessageSenderBadge::FullBadge { count: Some(5), .. }]
                ));
            }
            other => panic!("expected ChatMessage, got {other:?}"),
        }
    }

    #[test]
    fn parses_chat_message_with_simple_badge() {
        let frame = chat_message_frame(serde_json::json!([{"type": "vip", "text": "VIP"}]));
        let parsed: KickChatMessage = serde_json::from_str(&frame).unwrap();
        match parsed.data {
            MessageData::ChatMessage(m) => {
                assert!(matches!(
                    m.sender.identity.badges.as_slice(),
                    [ChatMessageSenderBadge::SimpleBadge { .. }]
                ));
            }
            other => panic!("expected ChatMessage, got {other:?}"),
        }
    }

    #[test]
    fn parses_connection_established() {
        let parsed: KickChatMessage = serde_json::from_str(&established_frame()).unwrap();
        match parsed.data {
            MessageData::PusherConnectionEstablished(data) => {
                assert_eq!(data.activity_timeout, 120);
            }
            other => panic!("expected PusherConnectionEstablished, got {other:?}"),
        }
    }

    #[test]
    fn parses_subscription_succeeded() {
        let parsed: KickChatMessage = serde_json::from_str(&subscribed_frame()).unwrap();
        assert!(matches!(
            parsed.data,
            MessageData::PusherSubscriptionSucceeded(_)
        ));
        assert_eq!(parsed.channel.as_deref(), Some("chatrooms.4598.v2"));
    }

    #[test]
    fn parses_ping_and_pong() {
        let ping: KickChatMessage = serde_json::from_str(&ping_frame()).unwrap();
        assert!(matches!(ping.data, MessageData::PusherPing(_)));

        let pong_frame = serde_json::json!({"event": "pusher:pong", "data": "{}"}).to_string();
        let pong: KickChatMessage = serde_json::from_str(&pong_frame).unwrap();
        assert!(matches!(pong.data, MessageData::PusherPong(_)));
    }

    #[test]
    fn parses_user_banned_and_unbanned() {
        let banned = serde_json::json!({
            "event": "App\\Events\\UserBannedEvent",
            "data": serde_json::json!({
                "id": "b1",
                "user": {"id": 1, "username": "a", "slug": "a"},
                "banned_by": {"id": 2, "username": "mod", "slug": "mod"},
                "permanent": true,
                "duration": null,
                "expires_at": null
            }).to_string()
        })
        .to_string();
        let parsed: KickChatMessage = serde_json::from_str(&banned).unwrap();
        assert!(matches!(parsed.data, MessageData::UserBanned(ref d) if d.permanent));

        let unbanned = serde_json::json!({
            "event": "App\\Events\\UserUnbannedEvent",
            "data": serde_json::json!({
                "id": "b1",
                "user": {"id": 1, "username": "a", "slug": "a"},
                "unbanned_by": {"id": 2, "username": "mod", "slug": "mod"},
                "permanent": true
            }).to_string()
        })
        .to_string();
        let parsed: KickChatMessage = serde_json::from_str(&unbanned).unwrap();
        assert!(matches!(parsed.data, MessageData::UserUnbanned(_)));
    }

    #[test]
    fn parses_chatroom_updated_and_clear() {
        let updated = serde_json::json!({
            "event": "App\\Events\\ChatroomUpdatedEvent",
            "data": serde_json::json!({
                "id": 1,
                "slow_mode": {"enabled": false, "message_interval": 1},
                "subscribers_mode": {"enabled": false},
                "followers_mode": {"enabled": true, "min_duration": 10},
                "emotes_mode": {"enabled": false},
                "advanced_bot_protection": {"enabled": false, "remaining_time": 0}
            }).to_string()
        })
        .to_string();
        let parsed: KickChatMessage = serde_json::from_str(&updated).unwrap();
        assert!(matches!(parsed.data, MessageData::ChatroomUpdated(_)));

        let clear = serde_json::json!({
            "event": "App\\Events\\ChatroomClearEvent",
            "data": serde_json::json!({"id": "1"}).to_string()
        })
        .to_string();
        let parsed: KickChatMessage = serde_json::from_str(&clear).unwrap();
        assert!(matches!(parsed.data, MessageData::ChatroomClear(_)));
    }

    #[test]
    fn parses_deleted_message() {
        let deleted = serde_json::json!({
            "event": "App\\Events\\MessageDeletedEvent",
            "data": serde_json::json!({
                "id": "d1",
                "message": {"id": "m1"},
                "aiModerated": true,
                "violatedRules": ["spam"]
            }).to_string()
        })
        .to_string();
        let parsed: KickChatMessage = serde_json::from_str(&deleted).unwrap();
        match parsed.data {
            MessageData::DeletedMessage(d) => {
                assert!(d.ai_moderated);
                assert_eq!(d.violated_rules.unwrap(), vec!["spam".to_string()]);
            }
            other => panic!("expected DeletedMessage, got {other:?}"),
        }
    }

    #[test]
    fn parses_poll_update_and_delete() {
        let update = serde_json::json!({
            "event": "App\\Events\\PollUpdateEvent",
            "data": serde_json::json!({
                "poll": {
                    "title": "Best game?",
                    "options": [{"id": 1, "label": "A", "votes": 3}],
                    "duration": 60,
                    "remaining": 30,
                    "result_display_duration": 5,
                    "has_voted": false,
                    "voted_option_id": null
                }
            }).to_string()
        })
        .to_string();
        let parsed: KickChatMessage = serde_json::from_str(&update).unwrap();
        assert!(matches!(parsed.data, MessageData::PollUpdate(_)));

        let delete = serde_json::json!({
            "event": "App\\Events\\PollDeleteEvent",
            "data": "{}"
        })
        .to_string();
        let parsed: KickChatMessage = serde_json::from_str(&delete).unwrap();
        assert!(matches!(parsed.data, MessageData::PollDelete(_)));
    }

    #[test]
    fn parses_pinned_message_events() {
        let created = serde_json::json!({
            "event": "App\\Events\\PinnedMessageCreatedEvent",
            "data": serde_json::json!({
                "message": {
                    "id": "m1",
                    "chatroom_id": 1,
                    "content": "hi",
                    "type": "message",
                    "created_at": "2026-01-01T00:00:00Z",
                    "sender": {"id": 1, "username": "a", "slug": "a", "identity": {"color": null, "badges": []}}
                },
                "duration": "60",
                "pinnedBy": {"id": 2, "username": "mod", "slug": "mod", "identity": {"color": null, "badges": []}}
            }).to_string()
        })
        .to_string();
        let parsed: KickChatMessage = serde_json::from_str(&created).unwrap();
        assert!(matches!(
            parsed.data,
            MessageData::PinnedMessageCreatedEvent(_)
        ));

        let deleted = serde_json::json!({
            "event": "App\\Events\\PinnedMessageDeletedEvent",
            "data": "{}"
        })
        .to_string();
        let parsed: KickChatMessage = serde_json::from_str(&deleted).unwrap();
        assert!(matches!(
            parsed.data,
            MessageData::PinnedMessageDeletedEvent(_)
        ));
    }

    #[test]
    fn parses_subscription_event() {
        let sub = serde_json::json!({
            "event": "App\\Events\\SubscriptionEvent",
            "data": serde_json::json!({"chatroom_id": 1, "username": "a", "months": 3}).to_string()
        })
        .to_string();
        let parsed: KickChatMessage = serde_json::from_str(&sub).unwrap();
        assert!(matches!(parsed.data, MessageData::SubscriptionEvent(_)));
    }

    #[test]
    fn unrecognized_event_fails_to_deserialize() {
        // MessageData has no catch-all variant for unmapped `event` tags -
        // read_message relies on this Err to build the `Unsupported` message.
        let unknown = serde_json::json!({
            "event": "App\\Events\\SomeBrandNewEvent",
            "data": "{}"
        })
        .to_string();
        assert!(serde_json::from_str::<KickChatMessage>(&unknown).is_err());
    }

    #[test]
    fn kick_error_display() {
        assert_eq!(
            KickError::StreamEnded.to_string(),
            "WebSocket stream ended unexpectedly"
        );
    }

    // ---- Behavioral tests against a local mock Pusher server ----

    /// Binds a plain-TCP mock server on an ephemeral port and hands each
    /// accepted connection (numbered from 1) to `handler`. Returns the `ws://`
    /// URL clients should connect to.
    async fn start_mock_server<H, Fut>(handler: H) -> String
    where
        H: Fn(WebSocketStream<TcpStream>, usize) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handler = Arc::new(handler);

        tokio::spawn(async move {
            let mut conn_num = 0usize;
            loop {
                let (stream, _) = match listener.accept().await {
                    Ok(v) => v,
                    Err(_) => break,
                };
                conn_num += 1;
                let ws = match accept_async(stream).await {
                    Ok(ws) => ws,
                    Err(_) => continue,
                };
                let h = handler.clone();
                tokio::spawn(async move { h(ws, conn_num).await });
            }
        });

        format!("ws://{}/", addr)
    }

    #[tokio::test]
    async fn forwards_connection_established_then_chat_message() {
        let url = start_mock_server(|mut ws, _n| async move {
            ws.send(Message::Text(established_frame().into()))
                .await
                .unwrap();
            let _ = ws.next().await; // consume the client's subscribe frame
            ws.send(Message::Text(
                chat_message_frame(serde_json::json!([])).into(),
            ))
            .await
            .unwrap();
            tokio::time::sleep(Duration::from_millis(200)).await;
        })
        .await;

        let mut client = KickClient::new(&url, vec![4598]).await.unwrap();

        let first = client.read_message().await.unwrap().unwrap();
        assert!(matches!(
            first.data,
            MessageData::PusherConnectionEstablished(_)
        ));

        let second = client.read_message().await.unwrap().unwrap();
        assert!(matches!(second.data, MessageData::ChatMessage(_)));
    }

    #[tokio::test]
    async fn swallows_ping_and_replies_with_pong() {
        let url = start_mock_server(|mut ws, _n| async move {
            ws.send(Message::Text(established_frame().into()))
                .await
                .unwrap();
            let _ = ws.next().await; // consume the client's subscribe frame

            ws.send(Message::Text(ping_frame().into())).await.unwrap();

            let reply = ws.next().await.unwrap().unwrap();
            match reply {
                Message::Text(t) => assert!(t.contains("pusher:pong")),
                other => panic!("expected a text pong reply, got {other:?}"),
            }

            ws.send(Message::Text(
                chat_message_frame(serde_json::json!([])).into(),
            ))
            .await
            .unwrap();
            tokio::time::sleep(Duration::from_millis(200)).await;
        })
        .await;

        let mut client = KickClient::new(&url, vec![4598]).await.unwrap();

        let first = client.read_message().await.unwrap().unwrap();
        assert!(matches!(
            first.data,
            MessageData::PusherConnectionEstablished(_)
        ));

        // The ping in between is swallowed internally; this should be the chat message.
        let second = client.read_message().await.unwrap().unwrap();
        assert!(matches!(second.data, MessageData::ChatMessage(_)));
    }

    #[tokio::test]
    async fn reconnects_after_unexpected_disconnect() {
        let url = start_mock_server(|mut ws, conn_num| async move {
            ws.send(Message::Text(established_frame().into()))
                .await
                .unwrap();
            let _ = ws.next().await; // consume the client's subscribe frame

            if conn_num == 1 {
                let _ = ws.close(None).await;
                return;
            }

            ws.send(Message::Text(
                chat_message_frame(serde_json::json!([])).into(),
            ))
            .await
            .unwrap();
            tokio::time::sleep(Duration::from_millis(200)).await;
        })
        .await;

        let mut client = KickClient::new(&url, vec![4598]).await.unwrap();

        let first = client.read_message().await.unwrap().unwrap();
        assert!(matches!(
            first.data,
            MessageData::PusherConnectionEstablished(_)
        ));

        // Connection #1 closes right after; read_message should reconnect
        // transparently and hand back connection #2's messages instead of
        // returning an error.
        let second = client.read_message().await.unwrap().unwrap();
        assert!(matches!(
            second.data,
            MessageData::PusherConnectionEstablished(_)
        ));

        let third = client.read_message().await.unwrap().unwrap();
        assert!(matches!(third.data, MessageData::ChatMessage(_)));
    }

    // ---- Reply metadata ----

    fn chat_frame(inner: serde_json::Value) -> String {
        serde_json::json!({
            "event": "App\\Events\\ChatMessageEvent",
            "data": inner.to_string()
        })
        .to_string()
    }

    #[test]
    fn parses_a_reply_with_numeric_sender_id() {
        let frame = chat_frame(serde_json::json!({
            "id": "abc",
            "chatroom_id": 1,
            "content": "yes exactly",
            "type": "reply",
            "created_at": "2026-09-09T10:00:00+00:00",
            "sender": {
                "id": 5, "username": "replier",
                "identity": {"color": "#fff", "badges": []}
            },
            "metadata": {
                "original_sender": {"id": 42, "username": "original"},
                "original_message": {"id": "parent-uuid", "content": "what do you think?"}
            }
        }));

        let parsed: KickChatMessage = serde_json::from_str(&frame).unwrap();
        let MessageData::ChatMessage(msg) = parsed.data else {
            panic!("expected a chat message");
        };

        assert_eq!(msg.r#type.as_deref(), Some("reply"));
        let meta = msg.metadata.expect("reply must carry metadata");
        assert_eq!(
            meta.original_message.unwrap().id.as_deref(),
            Some("parent-uuid")
        );
        let sender = meta.original_sender.unwrap();
        // Numeric ids are normalised to strings so callers get one type.
        assert_eq!(sender.id.as_deref(), Some("42"));
        assert_eq!(sender.username.as_deref(), Some("original"));
    }

    #[test]
    fn parses_a_reply_with_string_sender_id() {
        // Kick has sent this field both ways across API revisions.
        let frame = chat_frame(serde_json::json!({
            "id": "abc",
            "chatroom_id": 1,
            "content": "hi",
            "type": "reply",
            "sender": {"id": 5, "username": "r", "identity": {"color": null, "badges": []}},
            "metadata": {
                "original_sender": {"id": "42", "username": "original"},
                "original_message": {"id": "parent-uuid", "content": "x"}
            }
        }));

        let parsed: KickChatMessage = serde_json::from_str(&frame).unwrap();
        let MessageData::ChatMessage(msg) = parsed.data else {
            panic!("expected a chat message");
        };
        assert_eq!(
            msg.metadata.unwrap().original_sender.unwrap().id.as_deref(),
            Some("42")
        );
    }

    #[test]
    fn an_ordinary_message_has_no_metadata() {
        let frame = chat_frame(serde_json::json!({
            "id": "abc",
            "chatroom_id": 1,
            "content": "just talking",
            "type": "message",
            "sender": {"id": 5, "username": "u", "identity": {"color": null, "badges": []}}
        }));

        let parsed: KickChatMessage = serde_json::from_str(&frame).unwrap();
        let MessageData::ChatMessage(msg) = parsed.data else {
            panic!("expected a chat message");
        };
        assert_eq!(msg.r#type.as_deref(), Some("message"));
        assert!(msg.metadata.is_none());
    }

    #[test]
    fn a_reply_with_partial_metadata_still_parses() {
        // Missing sub-objects must not cost us the message itself.
        let frame = chat_frame(serde_json::json!({
            "id": "abc",
            "chatroom_id": 1,
            "content": "hi",
            "type": "reply",
            "sender": {"id": 5, "username": "u", "identity": {"color": null, "badges": []}},
            "metadata": {"original_message": {"id": "parent"}}
        }));

        let parsed: KickChatMessage = serde_json::from_str(&frame).unwrap();
        let MessageData::ChatMessage(msg) = parsed.data else {
            panic!("expected a chat message");
        };
        let meta = msg.metadata.unwrap();
        assert!(meta.original_sender.is_none());
        assert_eq!(meta.original_message.unwrap().id.as_deref(), Some("parent"));
    }

    // ---- Runtime subscription changes ----

    /// Collects every text frame the server receives, so tests can assert on
    /// the subscribe/unsubscribe traffic the client produced.
    fn frame_collector() -> (Arc<Mutex<Vec<String>>>, Arc<Mutex<Vec<String>>>) {
        let frames = Arc::new(Mutex::new(Vec::new()));
        (frames.clone(), frames)
    }

    #[tokio::test]
    async fn subscribe_sends_a_pusher_subscribe_frame_for_the_new_chatroom() {
        let (frames, sink) = frame_collector();

        let url = start_mock_server(move |mut ws, _| {
            let sink = sink.clone();
            async move {
                ws.send(Message::Text(established_frame().into())).await.unwrap();
                while let Some(Ok(Message::Text(text))) = ws.next().await {
                    sink.lock().await.push(text.to_string());
                }
            }
        })
        .await;

        let mut client = KickClient::new(&url, vec![100]).await.unwrap();
        let subs = client.subscriptions();

        // Drain the welcome so the connection is fully established.
        let _ = client.read_message().await.unwrap();

        subs.subscribe(200).await.unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;

        let seen = frames.lock().await.join("\n");
        assert!(
            seen.contains("chatrooms.200.v2"),
            "no subscribe frame for the new chatroom in: {}",
            seen
        );
        assert_eq!(subs.subscribed().await, vec![100, 200]);
    }

    #[tokio::test]
    async fn unsubscribe_sends_a_pusher_unsubscribe_frame() {
        let (frames, sink) = frame_collector();

        let url = start_mock_server(move |mut ws, _| {
            let sink = sink.clone();
            async move {
                ws.send(Message::Text(established_frame().into())).await.unwrap();
                while let Some(Ok(Message::Text(text))) = ws.next().await {
                    sink.lock().await.push(text.to_string());
                }
            }
        })
        .await;

        let mut client = KickClient::new(&url, vec![100, 200]).await.unwrap();
        let subs = client.subscriptions();
        let _ = client.read_message().await.unwrap();

        subs.unsubscribe(200).await.unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;

        let seen = frames.lock().await.join("\n");
        assert!(
            seen.contains("pusher:unsubscribe"),
            "no unsubscribe frame in: {}",
            seen
        );
        assert_eq!(subs.subscribed().await, vec![100]);
    }

    /// The bug this design exists to prevent: Pusher subscriptions do not
    /// survive a dropped socket, so a chatroom added at runtime has to be
    /// re-subscribed on reconnect or it goes silently unlogged.
    #[tokio::test]
    async fn chatrooms_added_at_runtime_are_resubscribed_after_a_reconnect() {
        let (frames, sink) = frame_collector();

        let url = start_mock_server(move |mut ws, conn_num| {
            let sink = sink.clone();
            async move {
                ws.send(Message::Text(established_frame().into())).await.unwrap();

                while let Some(Ok(msg)) = ws.next().await {
                    if let Message::Text(text) = msg {
                        // Only the second connection's frames matter: they show
                        // what was restored rather than originally requested.
                        if conn_num == 2 {
                            sink.lock().await.push(text.to_string());
                        }
                        // Drop connection 1 once the runtime subscribe lands.
                        if conn_num == 1 && text.contains("chatrooms.200.v2") {
                            let _ = ws.close(None).await;
                            return;
                        }
                    }
                }
            }
        })
        .await;

        let mut client = KickClient::new(&url, vec![100]).await.unwrap();
        let subs = client.subscriptions();
        let _ = client.read_message().await.unwrap();

        subs.subscribe(200).await.unwrap();

        // Drives the reconnect; the welcome frame comes from connection #2.
        let _ = client.read_message().await.unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;

        let seen = frames.lock().await.join("\n");
        assert!(
            seen.contains("chatrooms.100.v2"),
            "original chatroom not restored after reconnect: {}",
            seen
        );
        assert!(
            seen.contains("chatrooms.200.v2"),
            "runtime-added chatroom not restored after reconnect: {}",
            seen
        );
    }

    #[tokio::test]
    async fn repeated_subscribe_and_unknown_unsubscribe_are_noops() {
        let url = start_mock_server(|mut ws, _| async move {
            ws.send(Message::Text(established_frame().into())).await.unwrap();
            while ws.next().await.is_some() {}
        })
        .await;

        let mut client = KickClient::new(&url, vec![100]).await.unwrap();
        let subs = client.subscriptions();
        let _ = client.read_message().await.unwrap();

        // A caller polling for changes would otherwise accumulate duplicates,
        // and every duplicate would be re-subscribed on each reconnect.
        subs.subscribe(100).await.unwrap();
        subs.subscribe(100).await.unwrap();
        subs.unsubscribe(999).await.unwrap();

        assert_eq!(subs.subscribed().await, vec![100]);
    }

    // ---- Live network test (opt-in, hits the real kick.com API) ----

    #[tokio::test]
    #[ignore = "hits the real kick.com API; run with `cargo test -- --ignored`"]
    async fn resolve_chatroom_id_live() {
        let id = KickClient::resolve_chatroom_id("xyamii").await.unwrap();
        assert!(id > 0);
    }
}

#[cfg(test)]
mod reconnect_tests {
    use super::*;

    #[test]
    fn reconnect_delay_is_exponential_and_capped() {
        // The old linear ramp (1s, 2s, 3s...) spent its entire retry budget in
        // the first seconds of an outage. Exponential growth is what lets a
        // bounded attempt count still span a meaningful outage.
        assert_eq!(KickClient::reconnect_delay(1), Duration::from_secs(1));
        assert_eq!(KickClient::reconnect_delay(2), Duration::from_secs(2));
        assert_eq!(KickClient::reconnect_delay(3), Duration::from_secs(4));

        for attempt in 1..50 {
            assert!(
                KickClient::reconnect_delay(attempt) <= MAX_RECONNECT_DELAY,
                "attempt {} exceeded the delay ceiling",
                attempt
            );
        }
    }

    #[test]
    fn default_attempts_span_a_meaningful_outage() {
        // A logger must ride out more than a momentary blip before giving up
        // and forcing its supervisor to rebuild the client.
        let total: Duration = (1..=DEFAULT_MAX_RECONNECT_ATTEMPTS)
            .map(KickClient::reconnect_delay)
            .sum();

        assert!(
            total >= Duration::from_secs(60),
            "default retry budget only spans {:?}",
            total
        );
    }
}
