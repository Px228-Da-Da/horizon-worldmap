// The game's scroll clamp, `clamp_scroll`, with the mods' rows in the list's height
// while the page is laid out (the game cuts the scroll there before they are added).
fn worldmap_keys_clamp(ui: &mut OptionsUi, top: f32, bottom: f32, content: f32) {
    let max = (content + worldmap_keys::extra_inside() - (bottom - top)).max(0.0);
    ui.scroll = ui.scroll.clamp(0.0, max);
}
