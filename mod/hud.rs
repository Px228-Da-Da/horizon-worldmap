// The waypoints' markers in the world.
fn official_worldmap_hud(game: &crate::game::Game<'_>, batch: &mut horizon_client::hud::HudBatch) {
    crate::game::official_worldmap::hud(game, batch);
}
