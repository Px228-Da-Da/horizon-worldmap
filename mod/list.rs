// The mod's row in Options > Mods, with its keys.
fn official_worldmap_listed(mods: &mut Vec<crate::game::native_hooks::NativeMod>) {
    use crate::game::official_worldmap as m;
    mods.push(crate::game::native_hooks::NativeMod {
        id: m::ID.into(),
        title: format!("{} {}", m::NAME, m::VERSION),
        description: m::DESCRIPTION.into(),
        keys: m::KEYS
            .iter()
            .map(|k| [k.0.to_owned(), k.1.to_owned(), k.2.to_owned()])
            .collect(),
    });
}
