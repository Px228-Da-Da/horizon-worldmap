// The mod's settings page: its switches' labels.
fn official_worldmap_page(game: &mut crate::game::Game<'_>, mod_id: &str, buttons: &mut Vec<String>) {
    crate::game::official_worldmap::settings_page(game, mod_id, buttons);
}
