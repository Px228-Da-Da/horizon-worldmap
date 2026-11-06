// The map screen, drawn over the game.
fn official_worldmap_render(
    game: &crate::game::Game<'_>,
    batch: &mut horizon_client::hud::HudBatch,
    screen: &horizon_client::mod_api::NativeScreenView<'_>,
) {
    crate::game::official_worldmap::draw(game, batch, screen);
}
