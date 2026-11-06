// A mouse button went down on the map screen.
fn official_worldmap_press(
    game: &crate::game::Game<'_>,
    screen: &horizon_client::mod_api::NativeScreenView<'_>,
    button: i32,
    canceled: &mut bool,
) {
    crate::game::official_worldmap::pressed(game, screen, button, canceled);
}
