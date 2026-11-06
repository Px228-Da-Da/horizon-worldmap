// A key went down on the map screen: moving, zooming, waypoints, a name.
fn official_worldmap_screenkey(game: &crate::game::Game<'_>, key: &str, canceled: &mut bool) {
    crate::game::official_worldmap::screen_key(game, key, canceled);
}
