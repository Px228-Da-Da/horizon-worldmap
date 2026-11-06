// The mouse moved with a button down on the map screen: dragging the map.
fn official_worldmap_drag(
    game: &crate::game::Game<'_>,
    screen: &horizon_client::mod_api::NativeScreenView<'_>,
    button: i32,
    delta: [f32; 2],
    canceled: &mut bool,
) {
    crate::game::official_worldmap::dragged(game, screen, button, delta, canceled);
}
