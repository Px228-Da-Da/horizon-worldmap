// The wheel on the map screen: zoom.
fn official_worldmap_scroll(
    game: &crate::game::Game<'_>,
    screen: &horizon_client::mod_api::NativeScreenView<'_>,
    scroll: [f64; 2],
    canceled: &mut bool,
) {
    crate::game::official_worldmap::scrolled(game, screen, scroll, canceled);
}
