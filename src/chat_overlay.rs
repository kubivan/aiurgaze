use crate::controller::PlayerResources;
use crate::proxy_channel::PlayerId;
use bevy::prelude::*;
use sc2_proto::sc2api::{Request, Request_oneof_request};
pub const CHAT_MESSAGE_FRAMES: u32 = 112;
#[derive(Message, Clone)]
pub struct ChatMessageEvent {
    pub player_id: PlayerId,
    pub message: String,
}
#[derive(Clone)]
pub struct ActiveChatMessage {
    pub player_id: PlayerId,
    pub message: String,
}
#[derive(Resource, Default)]
pub struct ChatOverlay {
    pub active: Option<ActiveChatMessage>,
    pub expires_at_game_loop: u32,
}
pub fn chat_messages_from_request(player_id: PlayerId, raw: &[u8]) -> Vec<ChatMessageEvent> {
    let Ok(request) = <Request as protobuf::Message>::parse_from_bytes(raw) else {
        return Vec::new();
    };
    let Some(Request_oneof_request::action(action_request)) = request.request.as_ref() else {
        return Vec::new();
    };
    action_request
        .get_actions()
        .iter()
        .filter_map(|action| action.action_chat.as_ref())
        .map(|chat| chat.get_message())
        .filter(|message| !message.trim().is_empty())
        .map(|message| ChatMessageEvent {
            player_id,
            message: message.to_owned(),
        })
        .collect()
}
pub fn update_chat_overlay(
    mut events: MessageReader<ChatMessageEvent>,
    player_resources: Res<PlayerResources>,
    mut overlay: ResMut<ChatOverlay>,
) {
    for event in events.read() {
        overlay.active = Some(ActiveChatMessage {
            player_id: event.player_id,
            message: event.message.clone(),
        });
        overlay.expires_at_game_loop = player_resources
            .game_loop
            .saturating_add(CHAT_MESSAGE_FRAMES);
    }
    if player_resources.game_loop >= overlay.expires_at_game_loop {
        overlay.active = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protobuf::Message;
    use sc2_proto::sc2api::{Action, ActionChat};

    #[test]
    fn extracts_non_empty_action_chat_messages() {
        let mut request = Request::new();
        let mut chat_action = Action::new();
        let mut chat = ActionChat::new();
        chat.set_message("gg".to_owned());
        chat_action.set_action_chat(chat);
        request.mut_action().mut_actions().push(chat_action);

        let raw = request.write_to_bytes().unwrap();
        let events = chat_messages_from_request(PlayerId::Player2, &raw);

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].message, "gg");
        assert_eq!(events[0].player_id, PlayerId::Player2);
    }
}
