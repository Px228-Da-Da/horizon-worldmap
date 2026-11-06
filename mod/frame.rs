// Each frame: scan the terrain around the player, save now and then, and keep
// the map screen's image up to date while it is open.
fn official_worldmap_frame(game: &mut crate::game::Game<'_>) {
    crate::game::official_worldmap::frame(game);
}
