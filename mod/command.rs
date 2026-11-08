// A command from chat: the mod's own, which the green [Add] of a shared place runs.
fn official_worldmap_command(game: &mut crate::game::Game<'_>, command: &str, ran: &mut bool) {
    crate::game::official_worldmap::command(game, command, ran);
}
