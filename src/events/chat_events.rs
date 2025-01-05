use serde::de::Deserializer;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize, Debug)]
#[serde(untagged)]
pub enum MessageData {
    ChatMessage(ChatMessageEventData),
    DeletedMessage(DeletedMessageEventData),
    UserBanned(UserBannedEventData),
    UserUnbanned(UserUnbannedEventData),
    ChatroomUpdated(ChatroomUpdatedEventData),
    ChatroomClear(ChatroomClearEventData),
    PollUpdate(PollUpdateEventData),
    PollDelete(PollDeleteEventData),
    Unknown(String),
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ChatMessage {
    pub event: String,
    #[serde(deserialize_with = "deserialize_data")]
    pub data: Option<MessageData>,
    pub channel: String,
}

fn deserialize_data<'de, D>(deserializer: D) -> Result<Option<MessageData>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;

    match serde_json::from_value(value) {
        Ok(data) => Ok(Some(data)),
        Err(_) => Ok(None),
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ChatMessageEventData {
    pub id: String,
    pub chatroom_id: u32,
    pub content: Option<String>,
    pub r#type: Option<String>,
    pub created_at: Option<String>,
    pub sender: ChatMessageSender,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ChatMessageSender {
    id: u32,
    username: String,
    slug: String,
    identity: ChatMessageSenderIdentity,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ChatMessageSenderIdentity {
    color: Option<String>,
    badges: Vec<ChatMessageSenderBadge>,
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
    id: u32,
    user: User,
    banned_by: User,
    pernament: bool,
    duration: Option<u64>,
    expires_at: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct UserUnbannedEventData {
    id: u32,
    user: User,
    unbanned_by: User,
    pernament: bool,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct User {
    id: u32,
    username: String,
    slug: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ChatroomUpdatedEventData {
    id: u32,
    slow_mode: SlowMode,
    subscribers_mode: SubscribersMode,
    followers_mode: FollowersMode,
    emotes_mode: EmotesMode,
    advanced_bot_protection: AdvancedBotProtection,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct SlowMode {
    enabled: bool,
    message_interval: u64,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct SubscribersMode {
    enabled: bool,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct FollowersMode {
    enabled: bool,
    min_duration: u64,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct EmotesMode {
    enabled: bool,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct AdvancedBotProtection {
    enabled: bool,
    remaining_time: u64,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ChatroomClearEventData {
    id: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct DeletedMessageEventData {
    id: String,
    message: DeletedMessage,
    ai_moderated: bool,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct DeletedMessage {
    id: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct PollUpdateEventData {
    poll: Poll,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Poll {
    title: String,
    options: Vec<PollOption>,
    duration: u32,
    remaining: u32,
    result_display_duration: u32,
    has_voted: Option<bool>,
    voted_option_id: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct PollOption {
    id: u32,
    label: String,
    votes: u32,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct PollDeleteEventData {}

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

impl<'de> Deserialize<'de> for MessageData {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let data_str = String::deserialize(deserializer)?;
        match serde_json::from_str::<ChatMessageEventData>(&data_str) {
            Ok(chat_message) => return Ok(MessageData::ChatMessage(chat_message)),
            Err(_) => {}
        }

        match serde_json::from_str::<DeletedMessageEventData>(&data_str) {
            Ok(deleted_message) => return Ok(MessageData::DeletedMessage(deleted_message)),
            Err(_) => {}
        }

        match serde_json::from_str::<UserBannedEventData>(&data_str) {
            Ok(user_banned) => return Ok(MessageData::UserBanned(user_banned)),
            Err(_) => {}
        }

        match serde_json::from_str::<UserUnbannedEventData>(&data_str) {
            Ok(user_unbanned) => return Ok(MessageData::UserUnbanned(user_unbanned)),
            Err(_) => {}
        }

        match serde_json::from_str::<ChatroomUpdatedEventData>(&data_str) {
            Ok(chatroom_updated) => return Ok(MessageData::ChatroomUpdated(chatroom_updated)),
            Err(_) => {}
        }

        match serde_json::from_str::<ChatroomClearEventData>(&data_str) {
            Ok(chatroom_clear) => return Ok(MessageData::ChatroomClear(chatroom_clear)),
            Err(_) => {}
        }

        match serde_json::from_str::<PollUpdateEventData>(&data_str) {
            Ok(poll_update) => return Ok(MessageData::PollUpdate(poll_update)),
            Err(_) => {}
        }

        match serde_json::from_str::<PollDeleteEventData>(&data_str) {
            Ok(poll_delete) => return Ok(MessageData::PollDelete(poll_delete)),
            Err(_) => {}
        }

        Ok(MessageData::Unknown(data_str))
    }
}
