// The mod's row in Options > Mods, with its keys, in the game's language.
fn official_worldmap_listed(
    game: &crate::game::Game<'_>,
    mods: &mut Vec<crate::game::native_hooks::NativeMod>,
) {
    use crate::game::official_worldmap as m;
    let (title, description, keys) = m::listing(game);
    mods.push(crate::game::native_hooks::NativeMod {
        id: m::ID.into(),
        title,
        description,
        keys,
    });
}
