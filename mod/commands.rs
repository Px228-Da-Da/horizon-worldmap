// The names of the mod's own chat commands, for the chat box to colour, complete and list.
fn official_worldmap_commands(game: &crate::game::Game<'_>, commands: &mut Vec<&'static str>) {
    crate::game::official_worldmap::commands(game, commands);
}
