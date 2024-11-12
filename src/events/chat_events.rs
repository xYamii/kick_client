#[derive(Serialize, Deserialize, Debug)]
pub struct ChatMessageEventData {
    id: String,
    chatroom_id: u32,
    content: Option<String>,
    r#type: Option<String>,
    created_at: Option<String>,
    sender: ChatMessageSender
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ChatMessageSender {
    id: String,
    username: String,
    slug: String,
    identity: ChatMessageSenderIdentity
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ChatMessageSenderIdentity {
    color: Option<String>,
    badges: Vec<ChatMessageSenderBadges>
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ChatMessageSenderBadges {
    r#type: String,
    text: String
}

//todo implement all known badges type
#[derive(Serialize, Deserialize, Debug)]
pub enum ChatMessageSenderBadgesType {
    broadcaster,
    other
}

#[derive(Serialize, Deserialize, Debug)]
pub struct UserBannedEventData {
    id: String,
    user: User,
    banned_by: User,
    pernament: bool,
    duration: Option<u64>,
    expires_at: Option<String>
}

#[derive(Serialize, Deserialize, Debug)]
pub struct UserUnbannedEventData {
    id: String,
    user: User,
    unbanned_by: User,
    pernament: bool,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct User {
    id: String,
    username: String,
    slug: String
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ChatroomUpdatedEventData {
    id: u32,
    slow_mode: SlowMode,
    subscribers_mode: SubscribersMode,
    followers_mode: FollowersMode,
    emotes_mode: EmotesMode,
    advanced_bot_protection: AdvancedBotProtection
}

#[derive(Serialize, Deserialize, Debug)]
pub struct SlowMode {
    enabled: bool,
    message_interval: u64
}

#[derive(Serialize, Deserialize, Debug)]
pub struct SubscribersMode {
    enabled: bool,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct FollowersMode {
    enabled: bool,
    min_duration: u64
}

#[derive(Serialize, Deserialize, Debug)]
pub struct EmotesMode {
    enabled: bool,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct AdvancedBotProtection {
    enabled: bool,
    remaining_time: u64
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ChatroomClearEventData {
    id: String
}

#[derive(Serialize, Deserialize, Debug)]
pub struct PollUpdateEventData {
    poll: Poll
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Poll {
    title: String,
    options: Vec<PollOption>,
    duration: u32,
    remaining: u32,
    result_display_duration: u32,
    has_voted: Option<bool>,
    voted_option_id: Option<String>
}

#[derive(Serialize, Deserialize, Debug)]
pub struct PollOption{
    id: u32,
    label: String,
    votes: u32
}

#[derive(Serialize, Deserialize, Debug)]
pub struct PollDeleteEventData {
}