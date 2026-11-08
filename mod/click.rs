// A click on a button of the mod's page: a switch.
fn official_worldmap_click(game: &mut crate::game::Game<'_>, mod_id: &str, button: usize) {
    crate::game::official_worldmap::settings_click(game, mod_id, button);
}
