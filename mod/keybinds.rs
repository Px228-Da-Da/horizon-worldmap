// This mod's keys in Options > Controls > Key Binds (the other native mods' are left
// to their own pages): under the game's own, a heading with the mod's name, and a row for each of its keys, by label:
// the label, the button that picks the key and a Reset that puts the default back,
// as the game's own rows are. The page's widgets come back with those rows added
// and the list's height grown to hold them. A Reset is a click on the mod's
// button 1000 + the key's place (the mod's `settings_click` takes it).
fn worldmap_keybinds(
    value: (Vec<Widget>, f32, f32, f32),
    ui: &OptionsUi,
    s: &Settings,
    w: f32,
) -> (Vec<Widget>, f32, f32, f32) {
    if ui.page() != Page::KeyBinds {
        worldmap_keys::leave(0.0);
        return value;
    }
    // The native mods that have keys, by their place in the list.
    let with_keys: Vec<usize> = s
        .mods
        .installed
        .iter()
        .enumerate()
        .filter(|(_, m)| m.id == "worldmap" && m.native.as_ref().is_some_and(|p| !p.keys.is_empty()))
        .map(|(i, _)| i)
        .collect();
    if with_keys.is_empty() {
        worldmap_keys::leave(0.0);
        return value;
    }
    let (mut out, top, bottom, mut content) = value;
    let query = ui.settings_search.value.to_lowercase();
    let cx = (w / 2.0).floor();
    let row_left = cx - 170.0;
    let scrollbar_x = row_left + 340.0 + 6.0 + 2.0;
    let reset_x = scrollbar_x - 50.0 - 10.0;
    let change_x = reset_x - 5.0 - 75.0;
    // Under the lowest of the game's rows; the top of the list when it has none.
    let mut y = out
        .iter()
        .filter(|widget| widget.list)
        .map(|widget| widget.y + widget.h)
        .fold(top + 2.0 - ui.scroll, f32::max);
    let start = out.len();
    let first_y = y;
    for i in with_keys {
        let m = &s.mods.installed[i];
        let Some(page) = &m.native else {
            continue;
        };
        // Its keys that the search leaves, by label as the game sorts its own.
        let mut keys: Vec<(usize, &[String; 3])> = page
            .keys
            .iter()
            .enumerate()
            .filter(|(_, k)| query.is_empty() || k[1].to_lowercase().contains(&query))
            .collect();
        if keys.is_empty() {
            continue;
        }
        keys.sort_by(|a, b| a.1[1].cmp(&b.1[1]));
        // The mod's name, without the version its title ends with.
        let name = match m.title.rsplit_once(' ') {
            Some((name, version)) if version.chars().next().is_some_and(|c| c.is_ascii_digit()) => {
                name.to_owned()
            }
            _ => m.title.clone(),
        };
        out.push(Widget {
            x: 0.0,
            y: y + 7.0,
            w,
            h: 9.0,
            kind: WKind::Text {
                text: Text::literal(name).with_style(&Style {
                    bold: Some(true),
                    underlined: Some(true),
                    ..Style::default()
                }),
                color: u32::MAX,
                centered: true,
            },
            list: false,
            row: 0,
        });
        y += 20.0;
        let chosen = s.mods.keys.get(&m.id);
        for (k, [key_name, label, default]) in keys {
            let key = chosen.and_then(|c| c.get(key_name)).unwrap_or(default);
            let is_default = key == default;
            let mut message = keybinds::display_name(key);
            // The same key on another of the mod's, or on one of the game's.
            let shared = page.keys.iter().enumerate().any(|(j, other)| {
                j != k && chosen.and_then(|c| c.get(&other[0])).unwrap_or(&other[2]) == key
            }) || MAPPINGS.iter().any(|m| s.keys.get(m.0) == key.as_str());
            if shared {
                message = Text::literal("[ ")
                    .append(message.with_color(text::WHITE))
                    .append(" ]")
                    .with_color(text::YELLOW);
            }
            if ui.selected_native_key == Some((i, k)) {
                message = Text::literal("> ")
                    .append(message.with_style(&Style {
                        color: Some(text::WHITE),
                        underlined: Some(true),
                        ..Style::default()
                    }))
                    .append(" <")
                    .with_color(text::YELLOW);
            }
            out.push(Widget {
                x: row_left + 2.0,
                y: y + 6.0,
                w: 200.0,
                h: 9.0,
                kind: WKind::Text {
                    text: Text::literal(label.clone()),
                    color: u32::MAX,
                    centered: false,
                },
                list: false,
                row: 0,
            });
            out.push(button(change_x, y, 75.0, message, Act::NativeKey(i, k)));
            let reset = button(
                reset_x,
                y,
                50.0,
                tr("controls.reset"),
                Act::NativeButton(i, 1000 + k),
            );
            out.push(if is_default { inactive(reset) } else { reset });
            y += 20.0;
        }
    }
    if out.len() == start {
        worldmap_keys::leave(0.0);
        return (out, top, bottom, content);
    }
    // They scroll with the list, like the game's.
    for widget in &mut out[start..] {
        widget.list = true;
    }
    content = y + ui.scroll - top + 2.0;
    worldmap_keys::leave(y - first_y);
    (out, top, bottom, content)
}
