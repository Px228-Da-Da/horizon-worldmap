// A mouse button came up on the map screen.
fn official_worldmap_release(game: &crate::game::Game<'_>, canceled: &mut bool) {
    crate::game::official_worldmap::released(game, canceled);
}
