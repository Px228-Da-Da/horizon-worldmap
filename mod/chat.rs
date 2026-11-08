// A line of chat from the server: a place a player shared gets an [Add] button.
fn official_worldmap_chat(
    game: &crate::game::Game<'_>,
    message: &mut horizon_client::text::Text,
) {
    crate::game::official_worldmap::chat_received(game, message);
}
