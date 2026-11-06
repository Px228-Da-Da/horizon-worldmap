// The map's key opens and closes the map screen.
fn official_worldmap_key(game: &mut crate::game::Game<'_>, key: &str, pressed: bool) {
    crate::game::official_worldmap::key(game, key, pressed);
}
