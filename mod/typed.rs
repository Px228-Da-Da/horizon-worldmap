// A character typed on the map screen: part of a waypoint's name.
fn official_worldmap_typed(game: &crate::game::Game<'_>, c: char, canceled: &mut bool) {
    crate::game::official_worldmap::typed(game, c, canceled);
}
