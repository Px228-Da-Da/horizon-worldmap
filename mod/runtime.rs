// Horizon's World Map, a native mod: a full screen map of everything the player
// has explored, kept between visits, with waypoints. The terrain scan and the
// image are `horizon_client::minimap::worldmap_core` (the module this bundle adds
// to the minimap's file); this module is the game side: the hooks' state, the
// screen (drawn and driven through the mod screen hooks), the waypoints, and the
// markers drawn in the world. Its helper files call into it.
use super::Game;
use horizon_client::{
    hud::{HudBatch, MOD_IMAGE_SPRITE},
    minimap::worldmap_core::{DAY, FULL, LIVE, SURFACE, WorldMap, cave_layer, layer_top},
    mod_api::NativeScreenView,
    text::{ClickEvent, Style, Text},
};
use std::{
    borrow::Cow,
    cell::RefCell,
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

/// The mod's id (its settings and keys), name and version.
pub const ID: &str = "worldmap";
pub const NAME: &str = "World Map";
pub const VERSION: &str = "0.7.0";
pub const DESCRIPTION: &str = "A full screen map of everything you have explored, kept between visits: drag to move, scroll to zoom, right-click for a menu (teleport, waypoints, PNG export). Waypoints show in the world too.";
pub const KEY_NAME: &str = "open";
pub const DEFAULT_KEY: &str = "key.keyboard.m";

/// The mod's keys, as Options lists them: name, label and the default key. The
/// player's choices are kept by the game (`native_mod_key`).
pub const KEYS: [(&str, &str, &str); 16] = [
    ("open", "Open World Map", "key.keyboard.m"),
    ("waypoint", "New Waypoint", "key.keyboard.b"),
    ("caves", "Cave Mode Settings", "key.keyboard.c"),
    ("grid", "Toggle Chunk Grid", "key.keyboard.g"),
    ("center", "Center on Player", "key.keyboard.space"),
    ("zoom_in", "Zoom In (alternative)", "key.keyboard.equal"),
    ("zoom_out", "Zoom Out (alternative)", "key.keyboard.minus"),
    ("waypoints", "Toggle Waypoints", "key.keyboard.h"),
    ("goto", "Go to Coordinates", "key.keyboard.j"),
    ("list", "Waypoint List", "key.keyboard.l"),
    ("minimap", "Toggle Minimap", "key.keyboard.n"),
    ("minimap_zoom", "Minimap Zoom", "key.keyboard.z"),
    // In the menu of a waypoint (and on the one under the pointer).
    ("wp_edit", "Edit Waypoint", "key.keyboard.e"),
    ("wp_teleport", "Teleport to Waypoint", "key.keyboard.t"),
    ("wp_hide", "Disable Waypoint", "key.keyboard.h"),
    ("wp_delete", "Delete Waypoint", "key.keyboard.delete"),
];

/// CPU time the scan may take per frame.
const SCAN_BUDGET: Duration = Duration::from_micros(800);
/// The same while the map is open: the player is looking at it, so it may take more.
const SCAN_BUDGET_OPEN: Duration = Duration::from_micros(3000);
/// And while the minimap is cut at the player's height, which changes with every jump.
const SCAN_BUDGET_LIVE: Duration = Duration::from_micros(2500);
/// A region is written this often while some are waiting (a thread writes it), the rest at the
/// next time, so exploring fast does not write everything at once.
/// CPU time the files of a map may take in a frame.
const LOAD_BUDGET: Duration = Duration::from_millis(3);
const SAVE_EVERY: Duration = Duration::from_secs(2);
const ZOOM_MIN: f32 = 0.25;
const ZOOM_MAX: f32 = 64.0;
/// Waypoint colours, 0xRRGGBB, in the order new ones take.
const COLORS: [u32; 16] = [
    0xE04040, 0xF08030, 0xE0C030, 0x60D060, 0x20A050, 0x40C0A0, 0x40C0E0, 0x3070F0, 0x8060F0,
    0xC060E0, 0xF070B0, 0xFFFFFF, 0xB0B0B0, 0x606060, 0x804020, 0x80C0FF,
];
const SHAPES: usize = 3;
/// The dimensions the map can show, in the order the button goes through them.
const DIMS: [&str; 3] = [
    "minecraft:overworld",
    "minecraft:the_nether",
    "minecraft:the_end",
];

// The look: graphite panels, a teal accent, soft grey text.
const PANEL: u32 = 0xCC10_141C;
const EDGE: u32 = 0xFF2A_3342;
const HOT: u32 = 0xFF1F_3B44;
const ACCENT: u32 = 0x4FD1C5;
const TEXT: u32 = 0xE6EAF0;
const MUTED: u32 = 0x9AA5B5;
const WARN: u32 = 0xF2B84B;

/// The bottom toolbar: a label and a width each, and the height of every button.
const TOOLS: [(&str, f32); 9] = [
    ("", 24.0),
    ("", 24.0),
    ("Center", 64.0),
    ("Grid", 52.0),
    ("Caves", 60.0),
    ("Waypoint", 74.0),
    ("Export", 60.0),
    ("Dim", 100.0),
    ("List", 62.0),
];
const TOOL_H: f32 = 20.0;
const TOOL_GAP: f32 = 4.0;

/// The minimap: its sizes in GUI pixels, its scales in blocks to a pixel, and its corners.
const MINI_SIZES: [f32; 3] = [72.0, 96.0, 128.0];
const MINI_ZOOMS: [f32; 4] = [0.5, 1.0, 2.0, 4.0];
const MINI_CORNERS: [&str; 4] = ["Top left", "Top right", "Bottom left", "Bottom right"];
const MINI_SIZE_NAMES: [&str; 3] = ["Small", "Medium", "Large"];

/// The cave panel: its width, and the heights of its head, its rows and its help.
const CAVE_W: f32 = 222.0;
const CAVE_HEAD: f32 = 15.0;
const CAVE_ROW: f32 = 16.0;
const CAVE_HELP: f32 = 36.0;
const CAVE_ROWS: usize = 4;
/// The cave map's top can be set from the world's bottom to its top.
const TOP_MIN: i32 = -64;
const TOP_MAX: i32 = 319;

/// What each row of the cave panel does, in three short lines.
const CAVE_TEXT: [[&str; 3]; CAVE_ROWS] = [
    [
        "Auto: caves under a roof or in",
        "the Nether. On: always caves.",
        "Off: always the surface.",
    ],
    [
        "Layered: a cave map for each 16",
        "blocks of height. Full: one cave",
        "map for every height.",
    ],
    [
        "Highest block the cave map shows.",
        "Click, type a number, Enter.",
        "Auto: follow your height.",
    ],
    [
        "Drag to set the top of the cave",
        "map, from -64 to 319.",
        "Page Up/Down: 16 blocks at a time.",
    ],
];

/// The right-click menu: its width and the heights of its parts.
const MENU_W: f32 = 124.0;
const MENU_HEAD: f32 = 24.0;
const MENU_ROW: f32 = 13.0;
const MENU_FOOT: f32 = 13.0;

/// What the menu is for: a waypoint, or a place on the ground.
#[derive(Clone, Copy)]
enum MenuKind {
    Waypoint(usize),
    Ground { x: i32, z: i32 },
}

impl MenuKind {
    fn labels(self) -> &'static [&'static str] {
        match self {
            Self::Waypoint(_) => &[
                "Rename",
                "Teleport here",
                "Change color",
                "Change shape",
                "Group...",
                "Hide / show",
                "Share",
                "Delete",
            ],
            Self::Ground { .. } => &["Teleport here", "New waypoint here", "Export PNG"],
        }
    }
}

fn menu_height(rows: usize) -> f32 {
    MENU_HEAD + rows as f32 * MENU_ROW + MENU_FOOT
}

/// The row of a menu opened at `at` that the pointer is on.
fn menu_row_at(at: [f32; 2], width: f32, mouse: [f32; 2], rows: usize) -> Option<usize> {
    let (dx, dy) = (mouse[0] - at[0], mouse[1] - at[1] - MENU_HEAD);
    if dx < 0.0 || dx >= width || dy < 0.0 {
        return None;
    }
    let row = (dy / MENU_ROW) as usize;
    (row < rows).then_some(row)
}

/// A row of the waypoint list: a waypoint, or the heading of a group.
enum ListRow {
    Item(usize),
    Group(String),
}

/// The colours of the sixteen chat colours, which other map mods number their waypoints'.
const OTHER_COLORS: [u32; 16] = [
    0x000000, 0x0000AA, 0x00AA00, 0x00AAAA, 0xAA0000, 0xAA00AA, 0xFFAA00, 0xAAAAAA, 0x555555,
    0x5555FF, 0x55FF55, 0x55FFFF, 0xFF5555, 0xFF55FF, 0xFFFF55, 0xFFFFFF,
];

/// The colour of this mod's palette nearest to `rgb`.
fn nearest_color(rgb: u32) -> u32 {
    let part = |c: u32, s: u32| i64::from(c >> s & 255);
    let far = |c: u32| {
        (part(c, 16) - part(rgb, 16)).pow(2)
            + (part(c, 8) - part(rgb, 8)).pow(2)
            + (part(c, 0) - part(rgb, 0)).pow(2)
    };
    COLORS.iter().copied().min_by_key(|&c| far(c)).unwrap_or(COLORS[0])
}

/// A waypoint from a line of a waypoints file: this mod's (tab separated: name, x, y, z,
/// colour, dimension, shape, group), or another map mod's (`waypoint:name:initials:x:y:z:
/// colour:disabled:type:set:...`, with the dimension unsaid: `dim`).
fn parse_waypoint(line: &str, dim: &str) -> Option<Waypoint> {
    let line = line.trim_end_matches('\r');
    if let Some(rest) = line.strip_prefix("waypoint:") {
        let f: Vec<&str> = rest.split(':').collect();
        if f.len() < 6 {
            return None;
        }
        let group = f
            .get(8)
            .filter(|g| !g.is_empty() && !g.starts_with("gui."))
            .map(|g| g.replace("\u{a7}\u{a7}", ":"))
            .unwrap_or_default();
        return Some(Waypoint {
            name: f[0].replace("\u{a7}\u{a7}", ":"),
            x: f[2].parse().ok()?,
            y: f[3].parse().unwrap_or(64),
            z: f[4].parse().ok()?,
            color: nearest_color(OTHER_COLORS[f[5].parse::<usize>().ok()?.min(15)]),
            dim: dim.to_owned(),
            shape: 0,
            group,
            hidden: false,
        });
    }
    let f: Vec<&str> = line.split('\t').collect();
    if f.len() < 6 {
        return None;
    }
    let (Ok(x), Ok(y), Ok(z), Ok(color)) = (
        f[1].parse::<i32>(),
        f[2].parse::<i32>(),
        f[3].parse::<i32>(),
        u32::from_str_radix(f[4], 16),
    ) else {
        return None;
    };
    let shape = f.get(6).and_then(|s| s.parse::<u8>().ok()).unwrap_or(0);
    Some(Waypoint {
        name: f[0].to_owned(),
        x,
        y,
        z,
        color,
        dim: f[5].to_owned(),
        shape: shape % SHAPES as u8,
        group: f.get(7).map(|g| (*g).to_owned()).unwrap_or_default(),
        hidden: f.get(8) == Some(&"1"),
    })
}

struct Waypoint {
    name: String,
    x: i32,
    y: i32,
    z: i32,
    color: u32,
    dim: String,
    shape: u8,
    /// The group it is in, "" for none: a group can be hidden together.
    group: String,
    /// Hidden by itself (the menu's Disable).
    hidden: bool,
}

struct State {
    map: WorldMap,
    /// The world and dimension the map in memory is of.
    loaded: Option<(String, String)>,
    /// The world the waypoints are of.
    waypoints_of: Option<String>,
    dir: Option<PathBuf>,
    wp_file: Option<PathBuf>,
    waypoints: Vec<Waypoint>,
    wp_dirty: bool,
    /// The middle of the screen in blocks, and blocks per GUI pixel.
    cx: f64,
    cz: f64,
    bpp: f32,
    /// Where the zoom is going (it eases there), and the point of the screen and the
    /// block under it that stay together on the way.
    zoom_to: f32,
    zoom_anchor: Option<([f32; 2], (f64, f64))>,
    /// The window in GUI pixels, the pointer, and the image's size and
    /// how many GUI pixels each of its pixels covers.
    gui: (f32, f32),
    mouse: [f32; 2],
    img: (usize, usize, f32),
    /// The image's place: the block at its middle, blocks per image pixel, and its size.
    /// It is bigger than the screen, so the map moves within it without being drawn again.
    img_view: Option<(f64, f64, f32, usize, usize)>,
    /// The player: x, y, z, yaw; and where they have been lately.
    player: (f64, f64, f64, f32),
    /// How light the sky is, 0 (night) to `DAY` (full day); what the surface is drawn under.
    sky: u8,
    /// The world's name, its seed, and the folder they gave (read once, not every frame).
    key_memo: Option<(String, i64, String)>,
    /// A dragged map keeps going when let go, like a map on a phone: its speed in GUI
    /// pixels a second, the speed of the drag, when the last drag step was, and when the
    /// glide last moved.
    glide: (f64, f64),
    drag_vel: (f64, f64),
    drag_at: Option<Instant>,
    glide_at: Instant,
    /// The width of the box of the map's numbers at the bottom left (the cave panel keeps clear of it).
    info_w: f32,
    /// What the shared image of the mod holds: nothing, the map screen's, the minimap's.
    slot: u8,
    /// Underground, the player's feet (the minimap is cut at their head), NaN on the surface.
    live_anchor: f64,
    /// The minimap's scale, a place in `MINI_ZOOMS`.
    mini_zoom: usize,
    /// Where the minimap is centred and which way it faces: the camera between ticks, so it
    /// moves smoothly (x, z, yaw).
    mini_at: (f64, f64, f32),
    trail: Vec<(f64, f64)>,
    /// A waypoint's name being typed, and where it goes (`editing`: the waypoint
    /// being renamed, else a new one).
    entering: Option<(String, i32, i32)>,
    editing: Option<usize>,
    /// The colour picked for it, if one was.
    entering_color: Option<u32>,
    /// The open menu: for what, and where it is; and how wide it is, to fit its text.
    menu: Option<(MenuKind, [f32; 2])>,
    menu_w: f32,
    /// A chat command to send from the next frame (teleport).
    pending: Option<String>,
    /// The character a key press typed while it started the name: dropped.
    skip_char: Option<char>,
    /// The chunk grid is shown.
    grid: bool,
    /// The zoom slider is being dragged; a press that began on the toolbar or a
    /// menu does not move the map.
    slider_drag: bool,
    ui_press: bool,
    /// A line shown at the top for a few seconds.
    notice: Option<(String, Instant)>,
    /// Cave mode: 0 follows the player (under a roof, or in the Nether), 1 is
    /// always on, 2 never.
    cave_mode: u8,
    /// 0: a cave map for each 16 blocks of height (layered), 1: one for all heights.
    cave_type: u8,
    /// The highest block of the cave map picked by hand, else the player's height,
    /// and its text while it is being typed.
    top_y: Option<i32>,
    top_input: Option<String>,
    /// The cave panel is open, and its slider is being dragged.
    cave_panel: bool,
    cave_slider_drag: bool,
    /// The mod's settings, read once from the game's files.
    settings: Settings,
    settings_loaded: bool,
    /// The dimension the map shows when it is not the player's, and its map.
    view_dim: Option<String>,
    other: WorldMap,
    /// The world's map files, and the waypoint list: open, its search, typing, scroll.
    base: Option<PathBuf>,
    list_open: bool,
    list_query: String,
    list_focus: bool,
    list_scroll: f32,
    /// Waypoints are shown, the coordinates being typed for Go to, where the player was
    /// last alive, and whether they were dead a frame ago.
    show_waypoints: bool,
    /// The groups whose waypoints are hidden, and whether the name box holds a group.
    hidden_groups: std::collections::HashSet<String>,
    editing_group: bool,
    /// The waypoint the player is asked to confirm sharing.
    confirm_share: Option<usize>,
    goto: Option<String>,
    alive_at: (f64, f64, f64),
    was_dead: bool,
    /// The layer the map shows: `SURFACE` or a cave layer.
    view_layer: i32,
    /// Chunks the game has loaded that the map has not scanned yet.
    waiting: Vec<(i32, i32)>,
    born: Instant,
    last_save: Instant,
    /// The other players the game knows, now and then (copying every creature each frame costs):
    /// name and place, when it was made, and counters that let some work wait for a few frames.
    players: Vec<(String, [f64; 3])>,
    players_at: Instant,
    scan_tick: u32,
    key_tick: u32,
    /// Lines for the chat, said in the next frame.
    notes: Vec<String>,
    /// What a pixel of the map's image costs to draw (ms, smoothed), and how much coarser than
    /// the screen the image is drawn so that a slow computer is not held up by it (1: not).
    px_cost: f32,
    quality: f32,
    /// A map's files are being read, and the line in the log about it is still to be written.
    log_read: bool,
}

impl State {
    fn new() -> Self {
        Self {
            map: WorldMap::default(),
            loaded: None,
            waypoints_of: None,
            dir: None,
            wp_file: None,
            waypoints: Vec::new(),
            wp_dirty: false,
            cx: 0.0,
            cz: 0.0,
            bpp: 1.0,
            zoom_to: 1.0,
            zoom_anchor: None,
            gui: (480.0, 270.0),
            mouse: [0.0, 0.0],
            img: (0, 0, 1.0),
            img_view: None,
            player: (0.0, 64.0, 0.0, 0.0),
            sky: DAY,
            key_memo: None,
            glide: (0.0, 0.0),
            drag_vel: (0.0, 0.0),
            drag_at: None,
            glide_at: Instant::now(),
            info_w: 150.0,
            slot: 0,
            mini_zoom: 2,
            mini_at: (0.0, 0.0, 0.0),
            live_anchor: f64::NAN,
            trail: Vec::new(),
            entering: None,
            editing: None,
            entering_color: None,
            menu: None,
            menu_w: MENU_W,
            pending: None,
            skip_char: None,
            grid: false,
            slider_drag: false,
            ui_press: false,
            notice: None,
            cave_mode: 0,
            cave_type: 0,
            top_y: None,
            top_input: None,
            cave_panel: false,
            cave_slider_drag: false,
            settings: Settings::default(),
            settings_loaded: false,
            view_dim: None,
            other: WorldMap::default(),
            base: None,
            list_open: false,
            list_query: String::new(),
            list_focus: false,
            list_scroll: 0.0,
            show_waypoints: true,
            hidden_groups: std::collections::HashSet::new(),
            editing_group: false,
            confirm_share: None,
            goto: None,
            alive_at: (0.0, 64.0, 0.0),
            was_dead: false,
            view_layer: SURFACE,
            waiting: Vec::new(),
            born: Instant::now(),
            last_save: Instant::now(),
            players: Vec::new(),
            players_at: Instant::now(),
            scan_tick: 0,
            key_tick: 0,
            notes: Vec::new(),
            px_cost: 0.0,
            quality: 1.0,
            log_read: false,
        }
    }

    /// Something to say in the chat.
    fn note(&mut self, line: String) {
        if self.notes.len() < 8 {
            self.notes.push(line);
        }
    }

    /// A shared place's waypoint, added: the line to say about it.
    fn add_shared(&mut self, payload: &str) -> String {
        let Some(shared) = parse_payload(payload) else {
            return pick("That place is not valid", "Это место не распознано");
        };
        if self.waypoints_of.is_none() {
            return pick("Open a world first", "Сначала зайдите в мир");
        }
        let known = self.waypoints.iter().any(|w| {
            w.name == shared.name && w.x == shared.x && w.y == shared.y && w.z == shared.z && w.dim == shared.dim
        });
        if known {
            return pick(format!("You already have {}", shared.name), format!("Метка {} у вас уже есть", shared.name));
        }
        if self.waypoints.len() >= 2000 {
            return pick("Too many waypoints", "Слишком много меток");
        }
        let color = COLORS[self.waypoints.len() % COLORS.len()];
        let line = pick(
            format!("Waypoint {} added at {} {} {}", shared.name, shared.x, shared.y, shared.z),
            format!("Метка {} добавлена: {} {} {}", shared.name, shared.x, shared.y, shared.z),
        );
        self.waypoints.push(Waypoint {
            name: shared.name,
            x: shared.x,
            y: shared.y,
            z: shared.z,
            color,
            dim: shared.dim,
            shape: 0,
            group: String::new(),
            hidden: false,
        });
        self.wp_dirty = true;
        line
    }

    /// The waypoint a command means: its number in the list of this dimension, or its name.
    fn find_waypoint(&self, mine: &[usize], what: &str) -> Option<usize> {
        if let Ok(n) = what.parse::<usize>() {
            return mine.get(n.checked_sub(1)?).copied();
        }
        mine.iter().copied().find(|&i| self.waypoints[i].name.eq_ignore_ascii_case(what))
    }

    /// `/wp ...`: add, del, rename, list, share and clear, from the chat. The answer, line by
    /// line; a share leaves what is to be said in `pending`.
    fn wp_command(&mut self, args: &str) -> Vec<String> {
        let (verb, rest) = match args.trim().split_once(char::is_whitespace) {
            Some((v, r)) => (v.to_lowercase(), r.trim()),
            None => (args.trim().to_lowercase(), ""),
        };
        let help = || {
            vec![
                pick(
                    "/wp add [name] | del [name or number] | rename <number> <name> | list | share <number> <player or all> | clear",
                    "/wp add [имя] | del [имя или номер] | rename <номер> <имя> | list | share <номер> <игрок или all> | clear",
                ),
            ]
        };
        if verb.is_empty() || verb == "help" {
            return help();
        }
        if self.waypoints_of.is_none() {
            return vec![pick("Open a world first", "Сначала зайдите в мир")];
        }
        let dim = self.dim().to_owned();
        let (px, py, pz) = (self.player.0, self.player.1, self.player.2);
        let mine: Vec<usize> = self
            .waypoints
            .iter()
            .enumerate()
            .filter(|(_, w)| w.dim == dim)
            .map(|(i, _)| i)
            .collect();
        let away = |w: &Waypoint| {
            let (dx, dy, dz) = (f64::from(w.x) + 0.5 - px, f64::from(w.y) - py, f64::from(w.z) + 0.5 - pz);
            (dx * dx + dy * dy + dz * dz).sqrt()
        };
        let clean = |name: &str| -> String {
            name.chars().filter(|c| !c.is_control() && *c != '|' && *c != '"').take(32).collect::<String>().trim().to_owned()
        };
        match verb.as_str() {
            "add" | "set" => {
                let (bx, by, bz) = (px.floor() as i32, py.floor() as i32, pz.floor() as i32);
                let name = clean(rest);
                // On a waypoint: that one is renamed, they do not stack.
                let here = mine.iter().copied().find(|&i| {
                    let w = &self.waypoints[i];
                    w.x == bx && w.z == bz && (w.y - by).abs() <= 1
                });
                if let Some(i) = here {
                    if name.is_empty() {
                        return vec![pick(
                            format!("{} is already here", self.waypoints[i].name),
                            format!("Метка {} уже здесь", self.waypoints[i].name),
                        )];
                    }
                    self.waypoints[i].name = name.clone();
                    self.wp_dirty = true;
                    return vec![pick(format!("Renamed to {name}"), format!("Метка переименована: {name}"))];
                }
                if self.waypoints.len() >= 2000 {
                    return vec![pick("Too many waypoints", "Слишком много меток")];
                }
                let name = if name.is_empty() { format!("Waypoint {}", mine.len() + 1) } else { name };
                let color = COLORS[self.waypoints.len() % COLORS.len()];
                self.waypoints.push(Waypoint { name: name.clone(), x: bx, y: by, z: bz, color, dim, shape: 0, group: String::new(), hidden: false });
                self.wp_dirty = true;
                vec![pick(
                    format!("Waypoint {name} added at {bx} {by} {bz}"),
                    format!("Метка {name} добавлена: {bx} {by} {bz}"),
                )]
            }
            "del" | "delete" | "remove" => {
                let target = if rest.is_empty() {
                    mine.iter().copied().min_by(|&a, &b| away(&self.waypoints[a]).total_cmp(&away(&self.waypoints[b])))
                } else {
                    self.find_waypoint(&mine, rest)
                };
                let Some(i) = target else {
                    return vec![pick("No such waypoint", "Такой метки нет")];
                };
                let gone = self.waypoints.remove(i);
                self.wp_dirty = true;
                vec![pick(format!("Waypoint {} removed", gone.name), format!("Метка {} удалена", gone.name))]
            }
            "rename" => {
                let Some((what, name)) = rest.split_once(char::is_whitespace) else {
                    return help();
                };
                let name = clean(name);
                let Some(i) = self.find_waypoint(&mine, what).filter(|_| !name.is_empty()) else {
                    return vec![pick("No such waypoint", "Такой метки нет")];
                };
                self.waypoints[i].name = name.clone();
                self.wp_dirty = true;
                vec![pick(format!("Renamed to {name}"), format!("Метка переименована: {name}"))]
            }
            "list" => {
                if mine.is_empty() {
                    return vec![pick("No waypoints here", "Здесь нет меток")];
                }
                let mut lines: Vec<String> = mine
                    .iter()
                    .enumerate()
                    .take(30)
                    .map(|(n, &i)| {
                        let w = &self.waypoints[i];
                        format!("{}. {}  {} {} {}  {} m", n + 1, w.name, w.x, w.y, w.z, away(w).round() as i64)
                    })
                    .collect();
                if mine.len() > 30 {
                    lines.push(format!("... {}", mine.len() - 30));
                }
                lines
            }
            "share" => {
                let Some((what, who)) = rest.split_once(char::is_whitespace) else {
                    return help();
                };
                let who = who.trim();
                let Some(i) = self.find_waypoint(&mine, what) else {
                    return vec![pick("No such waypoint", "Такой метки нет")];
                };
                let line = share_line(&self.waypoints[i]);
                if who.eq_ignore_ascii_case("all") {
                    self.pending = Some(line);
                    return vec![pick("Shared with everyone", "Отправлено всем в чат")];
                }
                // A private message: it works on every server and only that player reads it.
                if who.is_empty() || who.len() > 32 || !who.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.')) {
                    return vec![pick("Say a player's name, or all", "Укажите ник игрока или all")];
                }
                self.pending = Some(format!("/msg {who} {line}"));
                vec![pick(format!("Sent to {who}"), format!("Отправлено игроку {who}"))]
            }
            "clear" => {
                if rest != "confirm" {
                    return vec![pick(
                        format!("This removes {} waypoints here: /wp clear confirm", mine.len()),
                        format!("Это удалит меток: {}. Подтвердите: /wp clear confirm", mine.len()),
                    )];
                }
                self.waypoints.retain(|w| w.dim != dim);
                self.wp_dirty = true;
                vec![pick("Waypoints removed", "Метки удалены")]
            }
            _ => help(),
        }
    }

    /// Write what changed.
    fn flush(&mut self) {
        if let Some(dir) = &self.dir
            && let Err(e) = self.map.save_dirty(dir, usize::MAX, false)
        {
            log::warn!("World Map: could not save the map: {e}");
        }
        if self.wp_dirty {
            self.save_waypoints();
        }
    }

    /// Every now and then: a region of the map, by another thread, and the waypoints.
    fn flush_a_little(&mut self) {
        if let Some(dir) = &self.dir {
            let _ = self.map.save_dirty(dir, 1, true);
        }
        if self.wp_dirty {
            self.save_waypoints();
        }
    }

    /// Not in a world: save, and let go of it.
    fn leave(&mut self) {
        self.key_memo = None;
        if self.loaded.is_some() {
            self.flush();
            self.map.clear();
            self.loaded = None;
            self.waypoints_of = None;
            self.entering = None;
            self.menu = None;
            self.trail.clear();
        }
    }

    /// The map in memory is of `key`'s dimension `dim`: load it when it is not.
    fn switch_world(&mut self, key: &str, dim: &str) {
        if self
            .loaded
            .as_ref()
            .is_some_and(|(k, d)| k == key && d == dim)
        {
            return;
        }
        self.flush();
        self.map.clear();
        self.trail.clear();
        self.loaded = Some((key.to_owned(), dim.to_owned()));
        let base = Game::native_config_dir()
            .ok()
            .map(|config| config.join("worldmap").join(key));
        if self.waypoints_of.as_deref() != Some(key) {
            move_old_maps(key);
        }
        self.dir = base.as_ref().map(|b| b.join(dim.replace(':', "_")));
        self.base = base.clone();
        self.view_dim = None;
        if self.waypoints_of.as_deref() != Some(key) {
            self.wp_file = base.map(|b| b.join("waypoints.txt"));
            self.waypoints_of = Some(key.to_owned());
            self.load_waypoints();
        }
        // The files are read a little in each frame (`load_step`), the nearest to the player first.
        let near = ((self.player.0.floor() as i32) >> 4, (self.player.2.floor() as i32) >> 4);
        if let Some(d) = &self.dir {
            self.map.begin_load(d, near);
            self.log_read = true;
        }
        log::info!("World Map: {key} {dim}: reading {} region files", self.map.loading().0);
    }

    fn load_waypoints(&mut self) {
        self.waypoints.clear();
        self.hidden_groups.clear();
        self.wp_dirty = false;
        let Some(path) = &self.wp_file else {
            return;
        };
        if let Ok(text) = std::fs::read_to_string(path.with_file_name("groups.txt")) {
            self.hidden_groups = text
                .lines()
                .filter(|l| !l.is_empty())
                .map(str::to_owned)
                .collect();
        }
        let Ok(text) = std::fs::read_to_string(path) else {
            return;
        };
        self.waypoints = text
            .lines()
            .filter_map(|line| parse_waypoint(line, "minecraft:overworld"))
            .collect();
    }

    /// The waypoints as the file keeps them, a line each, tab separated.
    fn waypoints_text(&self) -> String {
        let mut text = String::new();
        for w in &self.waypoints {
            text.push_str(&format!(
                "{}\t{}\t{}\t{}\t{:06x}\t{}\t{}\t{}\t{}\n",
                w.name,
                w.x,
                w.y,
                w.z,
                w.color,
                w.dim,
                w.shape,
                w.group,
                u8::from(w.hidden)
            ));
        }
        text
    }

    fn save_waypoints(&mut self) {
        self.wp_dirty = false;
        let Some(path) = &self.wp_file else {
            return;
        };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Err(e) = std::fs::write(path, self.waypoints_text()) {
            log::warn!("World Map: could not save the waypoints: {e}");
        }
        let groups_file = path.with_file_name("groups.txt");
        if self.hidden_groups.is_empty() {
            let _ = std::fs::remove_file(groups_file);
        } else {
            let mut hidden: Vec<&str> = self.hidden_groups.iter().map(String::as_str).collect();
            hidden.sort_unstable();
            let _ = std::fs::write(groups_file, hidden.join("\n") + "\n");
        }
    }

    /// Whether a waypoint is shown: not hidden itself, nor in a group that is hidden.
    fn group_visible(&self, w: &Waypoint) -> bool {
        !w.hidden && (w.group.is_empty() || !self.hidden_groups.contains(&w.group))
    }

    /// Write the waypoints to `exports/waypoints.txt` in the map's folder.
    fn export_waypoints(&mut self) {
        let Some(base) = self.wp_file.as_ref().and_then(|p| p.parent()) else {
            return;
        };
        let dir = base.join("exports");
        let result = std::fs::create_dir_all(&dir)
            .and_then(|()| std::fs::write(dir.join("waypoints.txt"), self.waypoints_text()));
        let n = self.waypoints.len();
        let line = match result {
            Ok(()) if ru() => format!("Метки ({n}) сохранены в exports/waypoints.txt"),
            Ok(()) => format!("{n} waypoints saved to exports/waypoints.txt"),
            Err(e) if ru() => format!("Не удалось сохранить метки: {e}"),
            Err(e) => format!("Could not save the waypoints: {e}"),
        };
        self.notice = Some((line, Instant::now()));
    }

    /// Read waypoints from `exports/import.txt` in the map's folder: this mod's own file
    /// (the one that is exported) or a waypoints file of another map mod; those that are
    /// already here are skipped.
    fn import_waypoints(&mut self) {
        let Some(base) = self.wp_file.as_ref().and_then(|p| p.parent()) else {
            return;
        };
        let path = base.join("exports").join("import.txt");
        let Ok(text) = std::fs::read_to_string(&path) else {
            let line = if ru() {
                "Положите файл меток в exports/import.txt в папке карты".to_owned()
            } else {
                "Put a waypoints file at exports/import.txt in the map's folder".to_owned()
            };
            self.notice = Some((line, Instant::now()));
            return;
        };
        let dim = self.view_dim_id().to_owned();
        let mut added = 0;
        for line in text.lines() {
            let Some(w) = parse_waypoint(line, &dim) else {
                continue;
            };
            let known = self
                .waypoints
                .iter()
                .any(|o| o.name == w.name && o.x == w.x && o.y == w.y && o.z == w.z && o.dim == w.dim);
            if !known {
                self.waypoints.push(w);
                added += 1;
            }
        }
        if added > 0 {
            self.wp_dirty = true;
        }
        let line = if ru() {
            format!("Добавлено меток: {added}")
        } else {
            format!("{added} waypoints added")
        };
        self.notice = Some((line, Instant::now()));
    }

    /// The screen is about to open on the player.
    fn open_at(&mut self, x: f64, z: f64) {
        self.cx = x;
        self.cz = z;
        self.entering = None;
        self.editing = None;
        self.menu = None;
        self.skip_char = None;
        self.slider_drag = false;
        self.ui_press = false;
        self.top_input = None;
        self.view_dim = None;
        self.list_focus = false;
        self.img_view = None;
        self.zoom_to = self.bpp;
        self.zoom_anchor = None;
        self.glide = (0.0, 0.0);
        self.drag_at = None;
        self.map.invalidate();
    }

    /// One step of the glide after a drag: the map slides on and slows down.
    fn glide_step(&mut self) {
        let now = Instant::now();
        let dt = now.duration_since(self.glide_at).as_secs_f64().min(0.05);
        self.glide_at = now;
        let speed = self.glide.0.hypot(self.glide.1);
        if speed < 12.0 {
            self.glide = (0.0, 0.0);
            return;
        }
        let bpp = f64::from(self.bpp);
        self.cx -= self.glide.0 * dt * bpp;
        self.cz -= self.glide.1 * dt * bpp;
        self.zoom_anchor = None;
        let keep = (-dt * 3.2).exp();
        self.glide = (self.glide.0 * keep, self.glide.1 * keep);
    }

    /// One step of the zoom's ease towards where it is going, a frame at a time.
    fn ease_zoom(&mut self) {
        if self.bpp == self.zoom_to {
            return;
        }
        let ratio = self.zoom_to / self.bpp;
        self.bpp = if (ratio - 1.0).abs() < 0.004 {
            self.zoom_to
        } else {
            self.bpp * ratio.powf(0.35)
        };
        if let Some((at, (wx, wz))) = self.zoom_anchor {
            let bpp = f64::from(self.bpp);
            self.cx = wx - f64::from(at[0] - self.gui.0 / 2.0) * bpp;
            self.cz = wz - f64::from(at[1] - self.gui.1 / 2.0) * bpp;
        }
        if self.bpp == self.zoom_to {
            self.zoom_anchor = None;
        }
    }

    /// The minimap's side in GUI pixels.
    fn mini_size(&self) -> f32 {
        MINI_SIZES[usize::from(self.settings.size).min(MINI_SIZES.len() - 1)]
    }

    /// Where the minimap is on a screen of `gw` x `gh`.
    fn mini_rect(&self, gw: f32, gh: f32) -> [f32; 4] {
        let size = self.mini_size();
        let margin = 8.0;
        let (x, y) = match self.settings.corner {
            0 => (margin, margin),
            // Under the status effects of the game.
            1 => (gw - size - margin, margin + 26.0),
            2 => (margin, gh - size - margin - 24.0),
            _ => (gw - size - margin, gh - size - margin - 24.0),
        };
        [x.round(), y.round(), size, size]
    }

    /// The minimap's image around the player, when it changed.
    fn mini_compose(&mut self) -> Option<(usize, Vec<u8>)> {
        let size = self.mini_size() as usize;
        if self.slot != 2 {
            self.map.invalidate();
        }
        let (px, pz, yaw) = self.mini_at;
        // Underground it is the floor at the player's height, else the surface.
        let layer = if self.live_anchor.is_finite() { LIVE } else { self.view_layer };
        let light = if self.settings.lighting && layer == SURFACE {
            self.sky
        } else {
            DAY
        };
        let bpp = MINI_ZOOMS[self.mini_zoom.min(MINI_ZOOMS.len() - 1)];
        let turned = self.settings.rotate.then_some(yaw);
        if self.map.compose(layer, px, pz, bpp, size, size, light, turned) {
            self.slot = 2;
            Some((size, self.map.image.clone()))
        } else {
            None
        }
    }

    /// The minimap in a corner of the game: the map round the player, turned the way they
    /// face (or north up), the waypoints (those off it on its edge), the sides of the world
    /// and the player's place.
    fn draw_minimap(&self, game: &Game<'_>, batch: &mut HudBatch) {
        if !self.settings.minimap || self.slot != 2 || !self.settings_loaded {
            return;
        }
        let (gw, gh) = (batch.width, batch.height);
        let [x, y, size, _] = self.mini_rect(gw, gh);
        // A frame: dark, a lighter edge, dark.
        game.native_fill(batch, [x - 3.0, y - 3.0, size + 6.0, size + 6.0], 0xFF0A_0C12);
        game.native_fill(batch, [x - 2.0, y - 2.0, size + 4.0, size + 4.0], 0xFF5A_6578);
        game.native_fill(batch, [x - 1.0, y - 1.0, size + 2.0, size + 2.0], 0xFF0A_0C12);
        let _ = game.native_sprite(batch, MOD_IMAGE_SPRITE, [0.0, 0.0, size, size], [x, y, size, size]);
        let bpp = MINI_ZOOMS[self.mini_zoom.min(MINI_ZOOMS.len() - 1)];
        let py = self.player.1;
        let (px, pz, yaw) = self.mini_at;
        let half = size / 2.0;
        let centre = (x + half, y + half);
        // Forward on the map: where the player faces when it turns, else north.
        let (sin, cos) = f64::from(yaw).to_radians().sin_cos();
        let (fx, fz) = if self.settings.rotate { (-sin, cos) } else { (0.0, -1.0) };
        // A direction of the world as the minimap shows it, right and down.
        let seen = |wx: f64, wz: f64| -> (f32, f32) {
            ((-fz * wx + fx * wz) as f32, (-(fx * wx + fz * wz)) as f32)
        };
        for w in self
            .waypoints
            .iter()
            .filter(|w| self.show_waypoints && w.dim == self.dim() && self.group_visible(w))
        {
            let (u, v) = seen(f64::from(w.x) + 0.5 - px, f64::from(w.z) + 0.5 - pz);
            let (mut mx, mut mz, mut r) = (u / bpp, v / bpp, 2.5);
            if mx.abs() > half - 3.0 || mz.abs() > half - 3.0 {
                // Off the minimap: on its edge, in the waypoint's direction.
                let k = (half - 3.0) / mx.abs().max(mz.abs());
                mx *= k;
                mz *= k;
                r = 1.8;
            }
            marker(game, batch, centre.0 + mx, centre.1 + mz, r, w.color, w.shape);
        }
        // The other players: a white diamond each, bright at the player's height and dim above
        // or below it.
        if self.settings.players {
            for (_, position) in &self.players {
                let (dx, dz) = (position[0] - px, position[2] - pz);
                let player = true;
                if dx.hypot(dz) < 1.2 {
                    continue;
                }
                let (u, v) = seen(dx, dz);
                let (mx, mz) = (u / bpp, v / bpp);
                if mx.abs() > half - 2.0 || mz.abs() > half - 2.0 {
                    continue;
                }
                let away = (position[1] - py).abs().min(24.0);
                let alpha = 255 - (away / 24.0 * 165.0) as u32;
                let (r, rgb) = if player { (2.6, 0xFFFFFF) } else { (2.0, 0xFFE04B) };
                let (cx, cy) = (centre.0 + mx, centre.1 + mz);
                game.native_quad(
                    batch,
                    [[cx, cy - r], [cx + r, cy], [cx, cy + r], [cx - r, cy]],
                    (alpha << 24) | rgb,
                );
            }
        }
        // The player: a small chevron, pointing up when the map turns.
        let f = if self.settings.rotate {
            [0.0, -1.0]
        } else {
            [-sin as f32, cos as f32]
        };
        let n = [-f[1], f[0]];
        let at = |a: f32, b: f32| [centre.0 + f[0] * a + n[0] * b, centre.1 + f[1] * a + n[1] * b];
        game.native_quad(
            batch,
            [at(6.0, 0.0), at(-3.5, 4.0), at(-1.0, 0.0), at(-3.5, -4.0)],
            0xFF00_0000 | ACCENT,
        );
        game.native_quad(
            batch,
            [at(4.0, 0.0), at(-2.2, 2.5), at(-0.4, 0.0), at(-2.2, -2.5)],
            0xFFFF_FFFF,
        );
        // The sides of the world, on the edge: north (red), east, south, west.
        let names: [&str; 4] = if ru() { ["С", "В", "Ю", "З"] } else { ["N", "E", "S", "W"] };
        for (name, (wx, wz)) in names.iter().zip([(0.0, -1.0), (1.0, 0.0), (0.0, 1.0), (-1.0, 0.0)]) {
            let (u, v) = seen(wx, wz);
            let k = (half - 6.0) / u.abs().max(v.abs());
            let (lx, ly) = (centre.0 + u * k, centre.1 + v * k);
            let fill = if *name == names[0] { 0xFFB0_3030 } else { 0xFF2B_3A6B };
            game.native_fill(batch, [lx - 5.0, ly - 5.0, 10.0, 10.0], 0xFF0A_0C12);
            game.native_fill(batch, [lx - 4.0, ly - 4.0, 8.0, 8.0], fill);
            centered_text(game, batch, name, lx, ly - 3.5, 0xFFFFFF);
        }
        // Where the player is, under the minimap (over it in the lower corners).
        let label = format!("{} {} {}", px.floor() as i32, py.floor() as i32, pz.floor() as i32);
        let w = width_of(game, &label);
        let ly = if self.settings.corner >= 2 { y - 12.0 } else { y + size + 5.0 };
        say(game, batch, &label, [(centre.0 - w / 2.0).round(), ly], TEXT, true);
    }

    /// The cave panel: `[x, y, width, height]`, over the toolbar.
    fn cave_rect(&self) -> [f32; 4] {
        let h = CAVE_HEAD + CAVE_ROWS as f32 * CAVE_ROW + CAVE_HELP;
        let centred = ((self.gui.0 - CAVE_W) / 2.0).round();
        let clear = (self.info_w + 14.0).min(self.gui.0 - CAVE_W - 6.0);
        [
            centred.max(clear),
            self.gui.1 - TOOL_H - 12.0 - h,
            CAVE_W,
            h,
        ]
    }

    /// The row of the cave panel a point is on.
    fn cave_row_at(&self, p: [f32; 2]) -> Option<usize> {
        let r = self.cave_rect();
        if !self.cave_panel || !inside(r, p) {
            return None;
        }
        let dy = p[1] - r[1] - CAVE_HEAD;
        if dy < 0.0 {
            return None;
        }
        let row = (dy / CAVE_ROW) as usize;
        (row < CAVE_ROWS).then_some(row)
    }

    /// The cave panel's slider track: its x and width.
    fn cave_track(&self) -> (f32, f32) {
        let r = self.cave_rect();
        (r[0] + 34.0, r[2] - 68.0)
    }

    /// The Auto button of the Top Y row.
    fn cave_auto_rect(&self) -> [f32; 4] {
        let r = self.cave_rect();
        [
            r[0] + r[2] - 48.0,
            r[1] + CAVE_HEAD + 2.0 * CAVE_ROW,
            40.0,
            CAVE_ROW - 2.0,
        ]
    }

    /// The top the cave map shows now: the one set, else the layer's, else the player's.
    fn effective_top(&self) -> i32 {
        self.top_y.unwrap_or(if self.view_layer == SURFACE || self.view_layer == FULL {
            self.player.1.floor() as i32
        } else {
            layer_top(self.view_layer)
        })
    }

    /// Set the top from where the pointer is on the slider.
    fn cave_slider_set(&mut self, mouse_x: f32) {
        let (x, w) = self.cave_track();
        let t = ((mouse_x - x) / w).clamp(0.0, 1.0);
        let v = TOP_MIN + (t * (TOP_MAX - TOP_MIN) as f32).round() as i32;
        self.top_input = None;
        if self.top_y != Some(v) {
            self.top_y = Some(v);
            self.map.invalidate();
        }
    }

    /// A row of the cave panel was clicked.
    fn cave_action(&mut self, row: usize) {
        match row {
            0 => self.cave_mode = (self.cave_mode + 1) % 3,
            1 => self.cave_type = (self.cave_type + 1) % 2,
            _ => {
                self.top_input = Some(self.top_y.map(|y| y.to_string()).unwrap_or_default());
            }
        }
        self.map.invalidate();
    }

    /// Go to: the numbers typed are X and Z (or X, Y and Z); the map centres there.
    fn commit_goto(&mut self) {
        let text = self.goto.take().unwrap_or_default();
        let numbers: Vec<i32> = text
            .split([' ', ','])
            .filter(|part| !part.is_empty())
            .filter_map(|part| part.parse().ok())
            .collect();
        let (x, z) = match numbers[..] {
            [x, z] | [x, _, z] => (x, z),
            _ => return,
        };
        self.cx = f64::from(x) + 0.5;
        self.cz = f64::from(z) + 0.5;
        self.zoom_anchor = None;
    }

    /// The player died: a waypoint where they were last alive.
    fn on_death(&mut self) {
        if self.was_dead || self.loaded.is_none() || !self.settings.death {
            return;
        }
        self.was_dead = true;
        let (x, y, z) = self.alive_at;
        let count = self
            .waypoints
            .iter()
            .filter(|w| w.name.starts_with("Death") || w.name.starts_with("Смерть"))
            .count();
        let dim = self.dim().to_owned();
        self.note(pick(format!("Waypoint Death set at {} {} {}", x.floor() as i32, y.floor() as i32, z.floor() as i32), format!("Метка Смерть поставлена: {} {} {}", x.floor() as i32, y.floor() as i32, z.floor() as i32)));
        self.waypoints.push(Waypoint {
            name: if count == 0 {
                tr("Death").into_owned()
            } else {
                format!("{} {}", tr("Death"), count + 1)
            },
            x: x.floor() as i32,
            y: y.floor() as i32,
            z: z.floor() as i32,
            color: 0xE04040,
            dim,
            shape: 2,
            group: String::new(),
            hidden: false,
        });
        self.wp_dirty = true;
    }

    /// The typed top Y was confirmed: a number, or nothing for auto.
    fn commit_top_y(&mut self) {
        let text = self.top_input.take().unwrap_or_default();
        self.top_y = text
            .trim()
            .parse::<i32>()
            .ok()
            .map(|y| y.clamp(TOP_MIN, TOP_MAX));
        self.map.invalidate();
    }

    /// The image, when the map or the view changed enough: the screen's, and round it as
    /// much margin as is left (the image holds 1024 pixels a side), so the map can be
    /// moved a good way before the image is drawn again. Dragging takes no drawing.
    fn compose(&mut self) -> Option<(usize, usize, Vec<u8>)> {
        let (gw, gh) = self.gui;
        if gw < 1.0 || gh < 1.0 {
            return None;
        }
        // While the map zooms its picture is drawn coarser (a ninth of the work): it changes
        // too fast to be seen as it is, and it is drawn as it is once the zoom has stopped.
        let base = (gw.max(gh) / 1024.0).ceil().max(1.0);
        self.quality = quality_for(self.px_cost, gw / base, gh / base, self.quality);
        let coarse = if self.bpp != self.zoom_to { self.quality.max(3.0) } else { self.quality };
        let scale = base * coarse;
        let (sw, sh) = ((gw / scale).ceil() as usize, (gh / scale).ceil() as usize);
        let pad_x = ((1024 - sw.min(1024)) / 2).min(160);
        let pad_y = ((1024 - sh.min(1024)) / 2).min(160);
        let (iw, ih) = (sw + 2 * pad_x, sh + 2 * pad_y);
        let ibpp = self.bpp * scale;
        let redo = match self.img_view {
            None => true,
            Some((ix, iz, b, w, h)) => {
                b != ibpp
                    || w != iw
                    || h != ih
                    || (self.cx - ix).abs() / f64::from(ibpp) > pad_x as f64 - 6.0
                    || (self.cz - iz).abs() / f64::from(ibpp) > pad_y as f64 - 6.0
            }
        };
        if redo {
            self.img_view = Some((self.cx, self.cz, ibpp, iw, ih));
        }
        self.img = (iw, ih, scale);
        let (ix, iz, b, w, h) = self.img_view?;
        let layer = self.shown_layer();
        // The surface of the world the player is in, under its sky: dark at night.
        let light = if self.settings.lighting && layer == SURFACE && !self.viewing_other() {
            self.sky
        } else {
            DAY
        };
        let started = Instant::now();
        if self.shown_mut().compose(layer, ix, iz, b, w, h, light, None) {
            let cost = started.elapsed().as_secs_f32() * 1000.0 / (w * h) as f32;
            self.px_cost = if self.px_cost == 0.0 { cost } else { self.px_cost * 0.7 + cost * 0.3 };
            self.slot = 1;
            Some((w, h, self.shown().image.clone()))
        } else {
            None
        }
    }

    /// Where block column (`x`, `z`) is on the screen, in GUI pixels.
    fn to_screen(&self, x: f64, z: f64) -> (f32, f32) {
        let bpp = f64::from(self.bpp);
        (
            ((x - self.cx) / bpp) as f32 + self.gui.0 / 2.0,
            ((z - self.cz) / bpp) as f32 + self.gui.1 / 2.0,
        )
    }

    /// The block column under a point of the screen.
    fn to_world(&self, p: [f32; 2]) -> (f64, f64) {
        let bpp = f64::from(self.bpp);
        (
            self.cx + f64::from(p[0] - self.gui.0 / 2.0) * bpp,
            self.cz + f64::from(p[1] - self.gui.1 / 2.0) * bpp,
        )
    }

    fn dim(&self) -> &str {
        self.loaded.as_ref().map_or("", |(_, d)| d.as_str())
    }

    /// What a menu's head says: two lines, the place it is for, and the colour of
    /// the first line.
    fn menu_head(&self, kind: MenuKind) -> (String, String, i32, i32, u32) {
        match kind {
            MenuKind::Waypoint(i) => match self.waypoints.get(i) {
                Some(w) => (
                    w.name.clone(),
                    format!("X {}   Y {}   Z {}", w.x, w.y, w.z),
                    w.x,
                    w.z,
                    w.color,
                ),
                None => (String::new(), String::new(), 0, 0, TEXT),
            },
            MenuKind::Ground { x, z } => (
                format!(
                    "X {x}   Y {}   Z {z}",
                    self.ground_y(x, z)
                        .map_or_else(|| "?".to_owned(), |y| y.to_string())
                ),
                if ru() {
                    format!("чанк {}, {}", x >> 4, z >> 4)
                } else {
                    format!("chunk {}, {}", x >> 4, z >> 4)
                },
                x,
                z,
                TEXT,
            ),
        }
    }

    /// The dimension shown is not the one the player is in.
    fn viewing_other(&self) -> bool {
        self.view_dim.as_deref().is_some_and(|d| d != self.dim())
    }

    /// The dimension shown.
    fn view_dim_id(&self) -> &str {
        self.view_dim.as_deref().unwrap_or_else(|| self.dim())
    }

    /// The map shown, and its layer (another dimension's is the surface).
    fn shown(&self) -> &WorldMap {
        if self.viewing_other() { &self.other } else { &self.map }
    }

    fn shown_mut(&mut self) -> &mut WorldMap {
        if self.viewing_other() { &mut self.other } else { &mut self.map }
    }

    fn shown_layer(&self) -> i32 {
        if self.viewing_other() { SURFACE } else { self.view_layer }
    }

    /// The next dimension's map, from its files when it is not the player's.
    fn cycle_dim(&mut self) {
        let now = self.view_dim_id().to_owned();
        let at = DIMS.iter().position(|d| *d == now).unwrap_or(0);
        let next = DIMS[(at + 1) % DIMS.len()];
        if next == self.dim() {
            self.view_dim = None;
        } else {
            self.view_dim = Some(next.to_owned());
            self.other.clear();
            if let Some(base) = &self.base {
                let near = ((self.cx.floor() as i32) >> 4, (self.cz.floor() as i32) >> 4);
                self.other.begin_load(&base.join(next.replace(':', "_")), near);
                log::info!("World Map: {next} shown, reading {} region files", self.other.loading().0);
            }
        }
        self.map.invalidate();
        self.other.invalidate();
        self.img_view = None;
        self.menu = None;
    }

    /// The waypoint list: its place, its search field.
    fn list_rect(&self) -> [f32; 4] {
        let top = 44.0;
        let bottom = self.gui.1 - TOOL_H - 12.0 - 44.0;
        [6.0, top, 176.0, (bottom - top).max(60.0)]
    }

    fn list_search_rect(&self) -> [f32; 4] {
        let r = self.list_rect();
        [r[0] + 6.0, r[1] + 17.0, r[2] - 12.0, 15.0]
    }

    /// The waypoints the list shows: of the dimension shown, that the search finds,
    /// the nearest first.
    fn list_items(&self) -> Vec<usize> {
        let query = self.list_query.to_lowercase();
        let dim = self.view_dim_id();
        let (ox, oz) = if self.viewing_other() {
            (self.cx, self.cz)
        } else {
            (self.player.0, self.player.2)
        };
        let mut found: Vec<(f64, usize)> = self
            .waypoints
            .iter()
            .enumerate()
            .filter(|(_, w)| {
                w.dim == dim && (query.is_empty() || w.name.to_lowercase().contains(&query))
            })
            .map(|(i, w)| {
                let d = (f64::from(w.x) - ox).powi(2) + (f64::from(w.z) - oz).powi(2);
                (d, i)
            })
            .collect();
        found.sort_by(|a, b| a.0.total_cmp(&b.0));
        found.into_iter().map(|f| f.1).collect()
    }

    /// The rows of the list: the waypoints, and when some are in groups a heading for each
    /// group (those with none last).
    fn list_rows(&self) -> Vec<ListRow> {
        let items = self.list_items();
        if items.iter().all(|&i| self.waypoints[i].group.is_empty()) {
            return items.into_iter().map(ListRow::Item).collect();
        }
        let mut groups: Vec<String> = items.iter().map(|&i| self.waypoints[i].group.clone()).collect();
        groups.sort_by_key(|g| (g.is_empty(), g.to_lowercase()));
        groups.dedup();
        let mut rows = Vec::new();
        for g in groups {
            rows.push(ListRow::Group(g.clone()));
            rows.extend(items.iter().filter(|&&i| self.waypoints[i].group == g).map(|&i| ListRow::Item(i)));
        }
        rows
    }

    /// The row of the list a point is on.
    fn list_row_at(&self, p: [f32; 2]) -> Option<ListRow> {
        let r = self.list_rect();
        let top = r[1] + 36.0;
        if !inside(r, p) || p[1] < top || p[1] > r[1] + r[3] - 20.0 {
            return None;
        }
        let row = ((p[1] - top + self.list_scroll) / 14.0) as usize;
        self.list_rows().into_iter().nth(row)
    }

    /// The list's Export and Import buttons, under its rows.
    fn list_buttons(&self) -> ([f32; 4], [f32; 4]) {
        let r = self.list_rect();
        let w = ((r[2] - 18.0) / 2.0).floor();
        let y = r[1] + r[3] - 17.0;
        ([r[0] + 6.0, y, w, 14.0], [r[0] + 12.0 + w, y, w, 14.0])
    }

    /// The name dialog: `[x, y, width, height]`, in the middle.
    fn dialog_rect(&self) -> [f32; 4] {
        let (w, h) = (300.0, 104.0);
        [
            ((self.gui.0 - w) / 2.0).round(),
            ((self.gui.1 - h) / 2.0).round(),
            w,
            h,
        ]
    }

    /// The dialog's colour squares, one for each colour of the palette.
    fn swatches(&self) -> [[f32; 4]; COLORS.len()] {
        let r = self.dialog_rect();
        let mut out = [[0.0; 4]; COLORS.len()];
        for (i, rect) in out.iter_mut().enumerate() {
            *rect = [r[0] + 12.0 + i as f32 * 17.0, r[1] + 45.0, 14.0, 14.0];
        }
        out
    }

    /// The colour the dialog shows picked: the one chosen, else the waypoint's own
    /// when it is being renamed, else the next one of the palette.
    fn dialog_color(&self) -> u32 {
        self.entering_color.unwrap_or_else(|| {
            self.editing
                .and_then(|i| self.waypoints.get(i))
                .map_or(COLORS[self.waypoints.len() % COLORS.len()], |w| w.color)
        })
    }

    /// The sharing confirmation: `[x, y, width, height]`, in the middle, and its Yes and No.
    fn confirm_rect(&self) -> [f32; 4] {
        let (w, h) = (300.0, 92.0);
        [
            ((self.gui.0 - w) / 2.0).round(),
            ((self.gui.1 - h) / 2.0).round(),
            w,
            h,
        ]
    }

    fn confirm_buttons(&self) -> ([f32; 4], [f32; 4]) {
        let r = self.confirm_rect();
        let y = r[1] + r[3] - 25.0;
        let no = [r[0] + r[2] / 2.0 + 4.0, y, 90.0, 18.0];
        let yes = [r[0] + r[2] / 2.0 - 4.0 - 90.0, y, 90.0, 18.0];
        (yes, no)
    }

    /// The answer to the sharing confirmation: yes sends the waypoint to the chat.
    fn answer_share(&mut self, yes: bool) {
        let Some(i) = self.confirm_share.take() else {
            return;
        };
        if yes && let Some(w) = self.waypoints.get(i) {
            self.pending = Some(share_line(w));
        }
    }

    /// The dialog's Save and Cancel buttons.
    fn dialog_buttons(&self) -> ([f32; 4], [f32; 4]) {
        let r = self.dialog_rect();
        let y = r[1] + r[3] - 25.0;
        let cancel = [r[0] + r[2] - 12.0 - 66.0, y, 66.0, 18.0];
        let save = [cancel[0] - 6.0 - 66.0, y, 66.0, 18.0];
        (save, cancel)
    }

    /// The height above the explored ground at a block column, if it is explored.
    fn ground_y(&self, x: i32, z: i32) -> Option<i32> {
        self.shown()
            .column(self.shown_layer(), x, z)
            .filter(|c| c.0 != 0)
            .map(|c| i32::from(c.1) + 1)
    }

    /// The waypoint under a point of the screen, the nearest within a few pixels.
    fn waypoint_at(&self, p: [f32; 2]) -> Option<usize> {
        if !self.show_waypoints {
            return None;
        }
        let mut best: Option<(usize, f32)> = None;
        for (i, w) in self.waypoints.iter().enumerate() {
            if w.dim != self.view_dim_id() || !self.group_visible(w) {
                continue;
            }
            let (x, y) = self.to_screen(f64::from(w.x) + 0.5, f64::from(w.z) + 0.5);
            let d = ((x - p[0]).powi(2) + (y - p[1]).powi(2)).sqrt();
            if d <= 8.0 && best.is_none_or(|b| d < b.1) {
                best = Some((i, d));
            }
        }
        best.map(|b| b.0)
    }

    fn zoom_at(&mut self, steps: f32, at: [f32; 2]) {
        // The point under `at` stays under it as the zoom eases to its new place.
        let world = self.to_world(at);
        self.zoom_to = (self.zoom_to * 1.25f32.powf(-steps)).clamp(ZOOM_MIN, ZOOM_MAX);
        self.zoom_anchor = Some((at, world));
    }

    /// The toolbar's buttons, `[x, y, width, height]`, centred at the bottom.
    /// The toolbar has no room for its labels: the icons alone.
    fn compact(&self) -> bool {
        let total: f32 =
            TOOLS.iter().map(|t| t.1).sum::<f32>() + TOOL_GAP * (TOOLS.len() as f32 - 1.0);
        total > self.gui.0 - 16.0
    }

    fn toolbar(&self) -> [[f32; 4]; TOOLS.len()] {
        let compact = self.compact();
        let width = |tool: &(&str, f32)| if compact { 24.0 } else { tool.1 };
        let total: f32 =
            TOOLS.iter().map(width).sum::<f32>() + TOOL_GAP * (TOOLS.len() as f32 - 1.0);
        let mut x = ((self.gui.0 - total) / 2.0).round();
        let y = self.gui.1 - TOOL_H - 6.0;
        let mut out = [[0.0; 4]; TOOLS.len()];
        for (rect, tool) in out.iter_mut().zip(TOOLS) {
            *rect = [x, y, width(&tool), TOOL_H];
            x += width(&tool) + TOOL_GAP;
        }
        out
    }

    fn tool_at(&self, p: [f32; 2]) -> Option<usize> {
        self.toolbar().iter().position(|r| inside(*r, p))
    }

    /// The zoom slider: its track `[x, y, width, height]`.
    fn slider(&self) -> [f32; 4] {
        [self.gui.0 - 16.0, self.gui.1 / 2.0 - 70.0, 8.0, 140.0]
    }

    fn slider_hit(&self, p: [f32; 2]) -> bool {
        let [x, y, w, h] = self.slider();
        inside([x - 6.0, y - 8.0, w + 12.0, h + 16.0], p)
    }

    /// Zoom to where the pointer is on the slider (the top is zoomed in).
    fn slider_set(&mut self, mouse_y: f32) {
        let [_, y, _, h] = self.slider();
        let t = ((mouse_y - y) / h).clamp(0.0, 1.0);
        self.bpp = ZOOM_MIN * (ZOOM_MAX / ZOOM_MIN).powf(t);
        self.zoom_to = self.bpp;
        self.zoom_anchor = None;
    }

    fn slider_value(&self) -> f32 {
        ((self.bpp / ZOOM_MIN).ln() / (ZOOM_MAX / ZOOM_MIN).ln()).clamp(0.0, 1.0)
    }

    fn tool_action(&mut self, tool: usize) {
        let centre = [self.gui.0 / 2.0, self.gui.1 / 2.0];
        match tool {
            0 => self.zoom_at(-1.0, centre),
            1 => self.zoom_at(1.0, centre),
            2 => {
                self.cx = self.player.0;
                self.cz = self.player.2;
            }
            3 => self.grid = !self.grid,
            4 => self.cave_panel = !self.cave_panel,
            5 => {
                self.editing = None;
                self.entering_color = None;
                self.entering = Some((
                    String::new(),
                    self.player.0.floor() as i32,
                    self.player.2.floor() as i32,
                ));
            }
            6 => self.export(),
            7 => self.cycle_dim(),
            _ => {
                self.list_open = !self.list_open;
                self.list_focus = false;
            }
        }
    }

    /// Save the whole explored map as a PNG beside the waypoints.
    fn export(&mut self) {
        let Some(base) = self.wp_file.as_ref().and_then(|p| p.parent()) else {
            return;
        };
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let name = format!("{}-{secs}.png", self.view_dim_id().trim_start_matches("minecraft:"));
        let path = base.join("exports").join(&name);
        let line = match self.shown().export_png(&path) {
            Ok((w, h)) if ru() => format!("Сохранено {name} ({w} x {h}) в папке карты"),
            Ok((w, h)) => format!("Saved {name} ({w} x {h}) in the map folder"),
            Err(e) if ru() => format!("Не удалось экспортировать: {e}"),
            Err(e) => format!("Could not export: {e}"),
        };
        self.notice = Some((line, Instant::now()));
    }

    /// An item of the open menu was chosen.
    fn menu_action(&mut self, kind: MenuKind, item: usize) {
        match kind {
            MenuKind::Ground { x, z } => match item {
                0 => self.teleport(x, self.ground_y(x, z).unwrap_or(self.player.1 as i32), z),
                1 => {
                    self.editing = None;
                    self.editing_group = false;
                    self.entering_color = None;
                    self.entering = Some((String::new(), x, z));
                }
                _ => self.export(),
            },
            MenuKind::Waypoint(i) => {
                if i >= self.waypoints.len() {
                    return;
                }
                match item {
                    0 => self.begin_edit(i),
                    1 => {
                        let w = &self.waypoints[i];
                        let (x, y, z, dim) = (w.x, w.y, w.z, w.dim.clone());
                        self.teleport_in(&dim, x, y, z);
                    }
                    2 => {
                        let w = &mut self.waypoints[i];
                        let at = COLORS.iter().position(|c| *c == w.color).unwrap_or(0);
                        w.color = COLORS[(at + 1) % COLORS.len()];
                        self.wp_dirty = true;
                    }
                    3 => {
                        let w = &mut self.waypoints[i];
                        w.shape = (w.shape + 1) % SHAPES as u8;
                        self.wp_dirty = true;
                    }
                    4 => self.begin_group(i),
                    5 => {
                        let w = &mut self.waypoints[i];
                        w.hidden = !w.hidden;
                        self.wp_dirty = true;
                    }
                    // Everyone in the chat will see it: ask first.
                    6 => self.confirm_share = Some(i),
                    _ => {
                        let gone = self.waypoints.remove(i);
                        self.wp_dirty = true;
                        self.note(pick(format!("Waypoint {} removed", gone.name), format!("Метка {} удалена", gone.name)));
                    }
                }
            }
        }
    }

    fn teleport(&mut self, x: i32, y: i32, z: i32) {
        let dim = self.view_dim_id().to_owned();
        self.teleport_in(&dim, x, y, z);
    }

    /// Go to a place of dimension `dim`: from another one the command names it.
    fn teleport_in(&mut self, dim: &str, x: i32, y: i32, z: i32) {
        let target = format!(
            "tp @s {} {} {}",
            f64::from(x) + 0.5,
            y,
            f64::from(z) + 0.5
        );
        self.pending = Some(if dim == self.dim() {
            format!("/{target}")
        } else {
            format!("/execute in {dim} run {target}")
        });
    }

    /// Rename waypoint `i`: its name in the box.
    fn begin_edit(&mut self, i: usize) {
        let w = &self.waypoints[i];
        self.entering = Some((w.name.clone(), w.x, w.z));
        self.editing = Some(i);
        self.editing_group = false;
        self.entering_color = None;
        self.skip_char = None;
    }

    /// Waypoint `i`'s group: its name in the box (empty: none).
    fn begin_group(&mut self, i: usize) {
        let w = &self.waypoints[i];
        self.entering = Some((w.group.clone(), w.x, w.z));
        self.editing = Some(i);
        self.editing_group = true;
        self.entering_color = None;
        self.skip_char = None;
    }

    fn begin_waypoint(&mut self, at: [f32; 2]) {
        let (x, z) = self.to_world(at);
        self.editing = None;
        self.editing_group = false;
        self.entering_color = None;
        self.entering = Some((String::new(), x.floor() as i32, z.floor() as i32));
    }

    fn commit_waypoint(&mut self) {
        let Some((name, x, z)) = self.entering.take() else {
            return;
        };
        let name = name.trim().to_owned();
        if self.editing_group {
            self.editing_group = false;
            self.entering_color = None;
            if let Some(w) = self.editing.take().and_then(|i| self.waypoints.get_mut(i)) {
                w.group = name;
                self.wp_dirty = true;
            }
            return;
        }
        let picked = self.entering_color.take();
        if let Some(i) = self.editing.take() {
            if let Some(w) = self.waypoints.get_mut(i) {
                if !name.is_empty() {
                    w.name = name;
                }
                if let Some(c) = picked {
                    w.color = c;
                }
                self.wp_dirty = true;
            }
            return;
        }
        let name = if name.is_empty() {
            format!("{} {}", tr("Waypoint"), self.waypoints.len() + 1)
        } else {
            name
        };
        let y = self.ground_y(x, z).unwrap_or(self.player.1 as i32);
        let color = picked.unwrap_or(COLORS[self.waypoints.len() % COLORS.len()]);
        let dim = self.view_dim_id().to_owned();
        self.note(pick(format!("Waypoint {name} set at {x} {y} {z}"), format!("Метка {name} поставлена: {x} {y} {z}")));
        self.waypoints.push(Waypoint {
            name,
            x,
            y,
            z,
            color,
            dim,
            shape: 0,
            group: String::new(),
            hidden: false,
        });
        self.wp_dirty = true;
    }

    fn typing_key(&mut self, key: &str) {
        match key {
            "key.keyboard.enter" | "key.keyboard.keypad.enter" => self.commit_waypoint(),
            "key.keyboard.escape" => {
                self.entering = None;
                self.editing = None;
                self.entering_color = None;
            }
            "key.keyboard.tab" => {
                let now = self.dialog_color();
                let at = COLORS.iter().position(|c| *c == now).unwrap_or(0);
                self.entering_color = Some(COLORS[(at + 1) % COLORS.len()]);
            }
            "key.keyboard.backspace" => {
                if let Some((name, _, _)) = &mut self.entering {
                    name.pop();
                }
            }
            _ => {}
        }
    }
}

fn inside(rect: [f32; 4], p: [f32; 2]) -> bool {
    p[0] >= rect[0] && p[0] < rect[0] + rect[2] && p[1] >= rect[1] && p[1] < rect[1] + rect[3]
}

/// Before each world had its own folder, every single player world's map was kept under
/// `mp-local`. The first world opened without a folder takes those maps (they are the
/// ones it was drawn on), and the old folder goes, so no map is lost or doubled.
fn move_old_maps(key: &str) {
    if !key.starts_with("sp-") {
        return;
    }
    let Ok(config) = Game::native_config_dir() else {
        return;
    };
    let old = config.join("worldmap").join("mp-local");
    let new = config.join("worldmap").join(key);
    if old.is_dir() && !new.exists() {
        match std::fs::rename(&old, &new) {
            Ok(()) => log::info!("World Map: the maps made before are now {key}'s"),
            Err(e) => log::warn!("World Map: could not move the old maps to {key}: {e}"),
        }
    }
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::new());
}

fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    STATE.with(|s| f(&mut s.borrow_mut()))
}

/// The name of a key as a short word for a menu (`key.keyboard.delete` is DEL).
fn key_label(key: &str) -> String {
    let k = key.strip_prefix("key.keyboard.").or_else(|| key.strip_prefix("key.")).unwrap_or(key);
    match k {
        "delete" => "DEL".to_owned(),
        "backspace" => "BKSP".to_owned(),
        "enter" => "ENTER".to_owned(),
        "space" => "SPACE".to_owned(),
        "insert" => "INS".to_owned(),
        "page_up" => "PGUP".to_owned(),
        "page_down" => "PGDN".to_owned(),
        "left.shift" | "right.shift" => "SHIFT".to_owned(),
        "left.control" | "right.control" => "CTRL".to_owned(),
        "left.alt" | "right.alt" => "ALT".to_owned(),
        "unknown" => String::new(),
        other => other.rsplit('.').next().unwrap_or(other).to_uppercase().chars().take(5).collect(),
    }
}

/// The hotkey in front of each row of a menu, as the player has the keys set.
fn menu_tags(game: &Game<'_>, kind: MenuKind) -> Vec<Option<String>> {
    match kind {
        MenuKind::Waypoint(_) => {
            let kb = keys(game);
            let tag = |k: &str| Some(key_label(k)).filter(|l| !l.is_empty());
            vec![
                tag(&kb.wp_edit),
                tag(&kb.wp_teleport),
                None,
                None,
                None,
                tag(&kb.wp_hide),
                None,
                tag(&kb.wp_delete),
            ]
        }
        MenuKind::Ground { .. } => vec![None; kind.labels().len()],
    }
}

/// Open the menu for `kind` at the pointer: as wide as its widest line, and kept on the
/// screen.
fn open_menu(game: &Game<'_>, s: &mut State, kind: MenuKind, mouse: [f32; 2]) {
    let (head1, head2, ..) = s.menu_head(kind);
    let mut width = MENU_W;
    let tags = menu_tags(game, kind);
    let tag_w = tags.iter().flatten().map(|t| width_of(game, t)).fold(0.0_f32, f32::max);
    let tag_w = if tag_w > 0.0 { tag_w + 5.0 } else { 0.0 };
    for line in [head1.as_str(), head2.as_str()] {
        width = width.max(width_of(game, line) + 14.0);
    }
    for line in kind.labels() {
        width = width.max(width_of(game, line) + tag_w + 14.0);
    }
    s.menu_w = width.min(s.gui.0 - 4.0);
    let height = menu_height(kind.labels().len());
    let at = [
        mouse[0].min(s.gui.0 - s.menu_w - 2.0).max(2.0),
        mouse[1].min(s.gui.1 - height - 2.0).max(2.0),
    ];
    s.menu = Some((kind, at));
}

/// Whether the map screen is the one open.
fn ours(game: &Game<'_>) -> bool {
    game.native_mod_screen() == Some(ID)
}

/// Whether a server address (`host:port`) is this computer.
fn is_loopback(address: &str) -> bool {
    if address == "::1" {
        return true;
    }
    let host = match address.strip_prefix('[') {
        Some(rest) => rest.split(']').next().unwrap_or(rest),
        None => address.rsplit_once(':').map_or(address, |(host, _)| host),
    };
    host.eq_ignore_ascii_case("localhost") || host == "::1" || host.starts_with("127.")
}

/// Worlds can have the same name ("New World" in three save folders), so the name alone
/// would give them one map. The folder of a single player world holds the seed of the world
/// that made it: the first world to use it writes its seed there, and a world with another
/// seed gets a folder of its own (`<name>-<seed>`).
fn world_folder(key: String, seed: i64) -> String {
    if !key.starts_with("sp-") || seed == 0 {
        return key;
    }
    let Ok(config) = Game::native_config_dir() else {
        return key;
    };
    let dir = config.join("worldmap").join(&key);
    let marker = dir.join("seed.txt");
    match std::fs::read_to_string(&marker) {
        Ok(text) if text.trim() == seed.to_string() => key,
        Ok(_) => format!("{key}-{seed:x}"),
        Err(_) => {
            // Not claimed yet: it is this world's (the maps made before took its name).
            move_old_maps(&key);
            if std::fs::create_dir_all(&dir).is_ok() {
                let _ = std::fs::write(&marker, seed.to_string());
            }
            key
        }
    }
}

/// The world's folder name for the map files: a single player world by its name and seed,
/// a friend's world by its seed, a server by its address.
fn world_key(game: &Game<'_>) -> String {
    let single_player = game
        .integrated
        .as_ref()
        .map(|i| i.world_name.clone())
        .filter(|name| !name.trim().is_empty())
        .or_else(|| {
            // A world run in the window itself: its level's name, else its folder's.
            game.native_world_dir().map(|path| {
                let level = game.level.name.to_string();
                if !level.trim().is_empty() {
                    return level;
                }
                path.file_name()
                    .and_then(|n| n.to_str())
                    .map(str::to_owned)
                    .unwrap_or_default()
            })
        });
    let raw = match single_player {
        Some(name) => format!("sp-{name}"),
        None => match game.native_client_state().and_then(|c| c.server_address) {
            // A friend's world (Play with Friends) is reached through a tunnel on this
            // computer, at an address that is the same for every friend: such a world is
            // told apart by its seed instead.
            Some(address) if is_loopback(&address) => match game.remote.as_ref() {
                Some(remote) => format!("friend-{:x}", remote.hashed_seed),
                None => format!("mp-{address}"),
            },
            Some(address) => format!("mp-{address}"),
            None => "local".to_owned(),
        },
    };
    // Letters of any language stay; what a file name cannot have goes.
    let clean: String = raw
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | ' ' | '(' | ')') {
                c
            } else {
                '_'
            }
        })
        .take(80)
        .collect();
    clean.trim_matches(|c| c == ' ' || c == '.').to_owned()
}


// -- Language ------------------------------------------------------------------

/// Whether the game is in Russian, read once a frame.
static RU: AtomicBool = AtomicBool::new(false);

fn ru() -> bool {
    RU.load(Ordering::Relaxed)
}

/// An interface text in the game's language: the Russian of a known text when the
/// game is in Russian, else the text.
fn tr(text: &str) -> Cow<'_, str> {
    if ru() && let Some(russian) = ru_text(text) {
        return Cow::Borrowed(russian);
    }
    Cow::Borrowed(text)
}

/// A line of text, in the game's language.
fn say(game: &Game<'_>, batch: &mut HudBatch, text: &str, at: [f32; 2], rgb: u32, shadow: bool) {
    game.native_text(batch, &tr(text), at, rgb, shadow);
}

fn width_of(game: &Game<'_>, text: &str) -> f32 {
    game.native_text_width(&tr(text))
}

/// A dimension's name as the interface says it.
fn dim_label(id: &str) -> String {
    match id {
        "minecraft:overworld" => tr("Overworld").into_owned(),
        "minecraft:the_nether" => tr("Nether").into_owned(),
        "minecraft:the_end" => tr("End").into_owned(),
        other => other.trim_start_matches("minecraft:").to_owned(),
    }
}

/// The third line of the top left panel: the biome under the pointer, in the game's
/// language, and where that is in the Nether or the Overworld.
fn biome_line(game: &Game<'_>, s: &State, x: i32, z: i32) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !s.viewing_other() {
        let y = s.ground_y(x, z).unwrap_or(s.player.1 as i32);
        let at = horizon_core::geometry::Vec3::new(
            f64::from(x) + 0.5,
            f64::from(y),
            f64::from(z) + 0.5,
        );
        if let Some(id) = game.biome_name_at(at) {
            let key = format!("biome.{}", id.replacen(':', ".", 1));
            let name = horizon_client::lang::get(&key);
            parts.push(if name == key {
                id.trim_start_matches("minecraft:").replace('_', " ")
            } else {
                name
            });
        }
    }
    match s.view_dim_id() {
        "minecraft:overworld" => parts.push(format!(
            "{} {}, {}",
            tr("Nether"),
            x.div_euclid(8),
            z.div_euclid(8)
        )),
        "minecraft:the_nether" => parts.push(format!("{} {}, {}", tr("Overworld"), x * 8, z * 8)),
        _ => {}
    }
    parts.join("  |  ")
}

/// The Russian of an interface text, if it has one.
fn ru_text(text: &str) -> Option<&'static str> {
    Some(match text {
        "Center" => "Центр",
        "Grid" => "Сетка",
        "Caves" => "Пещеры",
        "Waypoint" => "Метка",
        "Export" => "Экспорт",
        "Death" => "Смерть",
        "Overworld" => "Верхний мир",
        "Nether" => "Незер",
        "End" => "Энд",
        "surface" => "поверхность",
        "caves, every height" => "пещеры, все высоты",
        "Rename" => "Переименовать",
        "Teleport here" => "Телепорт сюда",
        "Change color" => "Сменить цвет",
        "Change shape" => "Сменить форму",
        "Delete" => "Удалить",
        "New waypoint here" => "Новая метка здесь",
        "Export PNG" => "Экспорт в PNG",
        "Rename waypoint" => "Переименование метки",
        "Save" => "Сохранить",
        "Cancel" => "Отмена",
        "Go" => "Перейти",
        "Go to coordinates: X Z" => "Перейти к координатам: X Z",
        "for example: 1200 -350" => "например: 1200 -350",
        "Cave map" => "Пещерная карта",
        "Top Y" => "Верх Y",
        "Auto" => "Авто",
        "Cave mode" => "Режим пещер",
        "Type" => "Тип",
        "auto" => "авто",
        "on" => "вкл",
        "off" => "выкл",
        "full" => "полный",
        "layered" => "слои",
        "Waypoints shown" => "Метки показаны",
        "Waypoints hidden" => "Метки скрыты",
        "N" => "С",
        "Auto: caves under a roof or in" => "Авто: пещеры под крышей и в",
        "the Nether. On: always caves." => "Незере. Вкл: всегда пещеры.",
        "Off: always the surface." => "Выкл: всегда поверхность.",
        "Layered: a cave map for each 16" => "Слои: карта на каждые 16 блоков",
        "blocks of height. Full: one cave" => "высоты. Полная: одна карта на",
        "map for every height." => "все высоты.",
        "Highest block the cave map shows." => "Верхний блок пещерной карты.",
        "Click, type a number, Enter." => "Нажмите, введите число, Enter.",
        "Auto: follow your height." => "Авто: следовать вашей высоте.",
        "Drag to set the top of the cave" => "Тяните, чтобы задать верх карты",
        "map, from -64 to 319." => "пещер, от -64 до 319.",
        "Page Up/Down: 16 blocks at a time." => "Page Up/Down: по 16 блоков.",
        "Player trail" => "След игрока",
        "Markers in the world" => "Метки в мире",
        "Waypoint on death" => "Метка при смерти",
        "Other players on the map" => "Другие игроки на карте",
        "Day and night on the map" => "День и ночь на карте",
        "Minimap" => "Миникарта",
        "Share" => "Поделиться",
        "Hide / show" => "Скрыть / показать",
        "Edit Waypoint" => "Изменить метку",
        "Teleport to Waypoint" => "Телепорт к метке",
        "Disable Waypoint" => "Скрыть метку",
        "Delete Waypoint" => "Удалить метку",
        "Rotate the minimap" => "Вращать миникарту",
        "Group..." => "Группа...",
        "Waypoint group" => "Группа метки",
        "No group" => "Без группы",
        "Import" => "Импорт",
        "Minimap corner" => "Угол миникарты",
        "Minimap size" => "Размер миникарты",
        "Top left" => "Слева сверху",
        "Top right" => "Справа сверху",
        "Bottom left" => "Слева снизу",
        "Bottom right" => "Справа снизу",
        "Small" => "Малая",
        "Medium" => "Средняя",
        "Large" => "Большая",
        "Toggle Minimap" => "Миникарта вкл/выкл",
        "Minimap Zoom" => "Масштаб миникарты",
        "Open World Map" => "Открыть карту мира",
        "New Waypoint" => "Новая метка",
        "Cave Mode Settings" => "Настройки пещер",
        "Toggle Chunk Grid" => "Сетка чанков",
        "Center on Player" => "К игроку",
        "Zoom In (alternative)" => "Приблизить (дополнительно)",
        "Zoom Out (alternative)" => "Отдалить (дополнительно)",
        "Toggle Waypoints" => "Показать или скрыть метки",
        "Go to Coordinates" => "Перейти к координатам",
        "Waypoint List" => "Список меток",
        "List" => "Список",
        "Waypoints" => "Метки",
        "Search" => "Поиск",
        _ => return None,
    })
}

// -- Settings ------------------------------------------------------------------

/// The settings page's switches: file name and label.
const SETTINGS: [(&str, &str); 7] = [
    ("trail", "Player trail"),
    ("markers", "Markers in the world"),
    ("death", "Waypoint on death"),
    ("players", "Other players on the map"),
    ("lighting", "Day and night on the map"),
    ("minimap", "Minimap"),
    ("minimap_rotate", "Rotate the minimap"),
];

/// The switches, all on until the player turns one off.
#[derive(Clone, Copy)]
struct Settings {
    trail: bool,
    markers: bool,
    death: bool,
    players: bool,
    /// The surface is dark at night, and lit blocks shine.
    lighting: bool,
    /// The minimap in the game, its corner (a place in `MINI_CORNERS`) and its size.
    minimap: bool,
    corner: u8,
    size: u8,
    /// The minimap turns with the player (the way they face is up), or north is up.
    rotate: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            trail: true,
            markers: true,
            death: true,
            players: true,
            lighting: true,
            minimap: true,
            corner: 0,
            size: 1,
            rotate: true,
        }
    }
}

impl Settings {
    fn get(&self, i: usize) -> bool {
        [self.trail, self.markers, self.death, self.players, self.lighting, self.minimap, self.rotate]
            .get(i)
            .copied()
            .unwrap_or(false)
    }

    fn set(&mut self, i: usize, on: bool) {
        match i {
            0 => self.trail = on,
            1 => self.markers = on,
            2 => self.death = on,
            3 => self.players = on,
            4 => self.lighting = on,
            5 => self.minimap = on,
            6 => self.rotate = on,
            _ => {}
        }
    }

    /// From the game's files for the mod's settings, one a switch.
    fn read(game: &Game<'_>) -> Self {
        let mut out = Self::default();
        for (i, (key, _)) in SETTINGS.iter().enumerate() {
            if let Some(value) = game.native_setting(ID, key) {
                out.set(i, value.trim() != "false");
            }
        }
        let number = |key: &str, max: usize, default: u8| {
            game.native_setting(ID, key)
                .and_then(|v| v.trim().parse::<usize>().ok())
                .map_or(default, |v| v.min(max) as u8)
        };
        out.corner = number("minimap_corner", MINI_CORNERS.len() - 1, out.corner);
        out.size = number("minimap_size", MINI_SIZES.len() - 1, out.size);
        out
    }
}

/// How light the sky is at a time of day: 0 at midnight to `DAY` at noon, by the way the game
/// moves the sun (its celestial angle), so dusk and dawn come when the sky darkens.
fn sky_level(day_time: i64) -> u8 {
    use std::f32::consts::{PI, TAU};
    let t = day_time.rem_euclid(24000) as f32 / 24000.0;
    let d = (t - 0.25).rem_euclid(1.0);
    let e = 0.5 - (d * PI).cos() / 2.0;
    let angle = d + (e - d) / 3.0;
    let dark = (1.0 - ((angle * TAU).cos() * 2.0 + 0.2)).clamp(0.0, 1.0);
    ((1.0 - dark) * f32::from(DAY)).round() as u8
}

// -- Hooks ---------------------------------------------------------------------

/// Every frame: scan around the player, save now and then, and while the map
/// is open draw its image and upload it when it changed.
pub fn frame(game: &mut Game<'_>) {
    if !game.native_on(ID) || !game.native_window() {
        return;
    }
    // Dead: a waypoint where they were last alive.
    let dead = game.screen == super::Screen::Dead;
    if dead {
        with(State::on_death);
    } else {
        with(|s| s.was_dead = false);
    }
    let Some(player) = game.native_player() else {
        with(State::leave);
        return;
    };
    if game.title.is_some() {
        return;
    }
    RU.store(game.settings.language.starts_with("ru"), Ordering::Relaxed);
    if !with(|s| s.settings_loaded) {
        let read = Settings::read(game);
        with(|s| {
            s.settings = read;
            s.settings_loaded = true;
        });
    }
    let cached = with(|s| {
        s.key_tick = s.key_tick.wrapping_add(1);
        if s.key_tick % 15 != 0 {
            s.key_memo.as_ref().map(|m| m.2.clone())
        } else {
            None
        }
    });
    let key = if let Some(known) = cached {
        known
    } else {
        let base = world_key(game);
        let seed = game.remote.as_ref().map_or(0, |r| r.hashed_seed);
        let known = with(|s| match &s.key_memo {
            Some((b, sd, k)) if *b == base && *sd == seed => Some(k.clone()),
            _ => None,
        });
        known.unwrap_or_else(|| {
            let key = world_folder(base.clone(), seed);
            with(|s| s.key_memo = Some((base, seed, key.clone())));
            key
        })
    };
    let open = ours(game);
    let gui = game.native_client_state().map(|c| c.gui_size);
    let radius = (game.settings.render_distance as i32).clamp(2, 16);
    let center = (
        (player.x.floor() as i32) >> 4,
        (player.z.floor() as i32) >> 4,
    );
    // The height of the player's eyes, and whether they are under a roof.
    let head = (player.y + 1.62).floor() as i32;
    let (bx, bz) = (player.x.floor() as i32, player.z.floor() as i32);
    let underground =
        player.dimension == "minecraft:the_nether" || game.world.light(bx, head, bz) >> 4 == 0;
    // The Nether and the End have no day and night.
    let sky = if player.dimension == "minecraft:the_nether" || player.dimension == "minecraft:the_end"
    {
        DAY
    } else {
        sky_level(game.world.day_time)
    };
    // The camera of the frame before, between ticks, else the player's place at the tick.
    let mini_at = game
        .locator_camera
        .map_or((player.x, player.z, player.yaw), |(at, yaw, _, _)| (at[0], at[2], yaw));
    let upload = with(|s| {
        s.player = (player.x, player.y, player.z, player.yaw);
        s.mini_at = mini_at;
        s.sky = sky;
        if !dead {
            s.alive_at = (player.x, player.y, player.z);
        }
        if let Some(gui) = gui {
            s.gui = (gui[0], gui[1]);
        }
        s.switch_world(&key, player.dimension);
        // A dot every few blocks of the way.
        let moved = s
            .trail
            .last()
            .is_none_or(|l| (l.0 - player.x).hypot(l.1 - player.z) >= 3.0);
        if moved && s.settings.trail {
            s.trail.push((player.x, player.z));
            if s.trail.len() > 40 {
                s.trail.remove(0);
            }
        }
        // Under a roof, in a cave or in the Nether the map is of the floor round the
        // player, in the layer of their height.
        let caves = match s.cave_mode {
            1 => true,
            2 => false,
            _ => underground,
        };
        // Layered: the player's 16 blocks of height; full: one map, down from their head.
        let slice = cave_layer(head);
        let scan_layer = caves.then(|| {
            if s.cave_type == 1 {
                (FULL, head)
            } else {
                (slice, layer_top(slice))
            }
        });
        s.view_layer = match (caves, s.cave_type) {
            (false, _) => SURFACE,
            (true, 1) => FULL,
            (true, _) => s.top_y.map_or(slice, cave_layer),
        };
        // The minimap's floor at the player's height, when they are under a roof or in a cave.
        let live = if caves && s.settings.minimap {
            // At every block of the player's height, a jump too.
            s.live_anchor = player.y;
            let turn = if s.settings.rotate { 1.42 } else { 1.0 };
            let blocks = s.mini_size() / 2.0 * MINI_ZOOMS[s.mini_zoom.min(MINI_ZOOMS.len() - 1)] * turn;
            let reach = ((blocks / 16.0).ceil() as i32 + 1).min(radius);
            Some((head, reach))
        } else {
            s.live_anchor = f64::NAN;
            None
        };
        // Looking at the map, or cut at the player's height: every frame; else every third.
        s.scan_tick = s.scan_tick.wrapping_add(1);
        let scan_now = open || live.is_some() || s.scan_tick % 3 == 0;
        if scan_now {
        // The files of the map, a little in each frame (a big map must not hold up the game).
        let reading = s.map.load_step(LOAD_BUDGET);
        s.other.load_step(LOAD_BUDGET);
        if !reading && s.log_read {
            s.log_read = false;
            log::info!("World Map: {} explored chunks read", s.map.loading().1);
        }
        s.map.update(
            &game.world,
            game.res.facts(),
            center,
            radius,
            if open {
                SCAN_BUDGET_OPEN
            } else if live.is_some() {
                SCAN_BUDGET_LIVE
            } else {
                SCAN_BUDGET
            },
            scan_layer,
            live,
        );
        }
        s.waiting = if open && s.view_layer == scan_layer.map_or(SURFACE, |c| c.0) {
            s.map
                .pending(&game.world, center, radius, s.view_layer, 96)
        } else {
            Vec::new()
        };
        if s.last_save.elapsed() >= SAVE_EVERY {
            s.last_save = Instant::now();
            s.flush_a_little();
        }
        if open {
            s.ease_zoom();
            s.glide_step();
            s.compose()
        } else {
            None
        }
    });
    if let Some((w, h, rgba)) = upload {
        game.native_mod_image(w as u32, h as u32, &rgba);
    }
    // The other players, four times a second (the game copies every creature to tell them).
    let want_players = with(|s| {
        s.settings.players
            && (s.settings.minimap || open)
            && s.players_at.elapsed() >= Duration::from_millis(250)
    });
    if want_players {
        let list: Vec<(String, [f64; 3])> = game
            .native_client_entities()
            .into_iter()
            .filter(|e| e.alive && !e.invisible)
            .filter_map(|e| e.player_name.map(|name| (name, e.position)))
            .collect();
        with(|s| {
            s.players = list;
            s.players_at = Instant::now();
        });
    } else if with(|s| !s.settings.players && !s.players.is_empty()) {
        with(|s| s.players.clear());
    }
    // The minimap shares the image with the map screen: it draws its own when the screen is shut.
    let mini = with(|s| {
        if s.settings.minimap && !open && !dead {
            s.mini_compose()
        } else {
            None
        }
    });
    if let Some((size, rgba)) = mini {
        game.native_mod_image(size as u32, size as u32, &rgba);
    }
    // What the map has to tell, in the chat.
    for line in with(|s| std::mem::take(&mut s.notes)) {
        chat(game, &line);
    }
    // A teleport the menu asked for: close the map, then the command.
    if let Some(command) = with(|s| s.pending.take()) {
        game.native_mod_screen_close(ID);
        if let Err(e) = game.native_send_chat(&command) {
            log::warn!("World Map: could not send {command}: {e}");
        }
    }
}

/// A key went down: the map's own opens and closes it.
pub fn key(game: &mut Game<'_>, key: &str, pressed: bool) {
    if !pressed || !game.native_on(ID) {
        return;
    }
    if game.native_playing() && game.native_window() && !ours(game) {
        let name_of = |name: &str| {
            let default = KEYS.iter().find(|k| k.0 == name).map_or("", |k| k.2);
            game.native_mod_key(ID, name, default)
        };
        if key == name_of("minimap") {
            let on = !with(|s| s.settings.minimap);
            game.native_set_setting(ID, "minimap", if on { "true" } else { "false" });
            with(|s| {
                s.settings.minimap = on;
                s.slot = 0;
            });
            return;
        }
        if key == name_of("minimap_zoom") {
            with(|s| {
                s.mini_zoom = (s.mini_zoom + 1) % MINI_ZOOMS.len();
                s.map.invalidate();
            });
            return;
        }
    }
    if key != game.native_mod_key(ID, KEY_NAME, DEFAULT_KEY) {
        return;
    }
    if ours(game) {
        // Typing a name, the key is a letter.
        let free = with(|s| {
            s.entering.is_none() && s.top_input.is_none() && s.goto.is_none() && !s.list_focus
        });
        if free {
            game.native_mod_screen_close(ID);
            with(State::flush);
        }
        return;
    }
    if !game.native_playing() || !game.native_window() {
        return;
    }
    let Some(player) = game.native_player() else {
        return;
    };
    with(|s| s.open_at(player.x, player.z));
    game.native_mod_screen_open(ID);
}

/// A mouse button went down on the map screen.
pub fn pressed(game: &Game<'_>, screen: &NativeScreenView<'_>, button: i32, canceled: &mut bool) {
    if !ours(game) {
        return;
    }
    *canceled = true;
    with(|s| {
        let mouse = screen.mouse;
        s.mouse = mouse;
        s.glide = (0.0, 0.0);
        s.drag_vel = (0.0, 0.0);
        s.drag_at = None;
        s.slider_drag = false;
        s.cave_slider_drag = false;
        s.ui_press = false;
        if s.goto.is_some() {
            s.ui_press = true;
            if button == 0 {
                let (go, cancel) = s.dialog_buttons();
                if inside(go, mouse) {
                    s.commit_goto();
                } else if inside(cancel, mouse) {
                    s.goto = None;
                }
            }
            return;
        }
        if s.entering.is_some() {
            s.ui_press = true;
            if button == 0 {
                let (save, cancel) = s.dialog_buttons();
                if let Some(i) = s.swatches().iter().position(|r| inside(*r, mouse)) {
                    s.entering_color = Some(COLORS[i]);
                } else if inside(save, mouse) {
                    s.commit_waypoint();
                } else if inside(cancel, mouse) {
                    s.entering = None;
                    s.editing = None;
                    s.entering_color = None;
                }
            }
            return;
        }
        if s.confirm_share.is_some() {
            s.ui_press = true;
            if button == 0 {
                let (yes, no) = s.confirm_buttons();
                if inside(yes, mouse) {
                    s.answer_share(true);
                } else if inside(no, mouse) {
                    s.answer_share(false);
                }
            }
            return;
        }
        // An open menu takes the click: a row, or just closes.
        if let Some((kind, at)) = s.menu.take() {
            s.ui_press = true;
            if button == 0
                && let Some(row) = menu_row_at(at, s.menu_w, mouse, kind.labels().len())
            {
                s.menu_action(kind, row);
            }
            return;
        }
        // The waypoint list: its search, and its rows.
        if s.list_open && inside(s.list_rect(), mouse) {
            s.ui_press = true;
            if inside(s.list_search_rect(), mouse) {
                s.list_focus = button == 0;
            } else {
                s.list_focus = false;
                let (export, import) = s.list_buttons();
                if button == 0 && inside(export, mouse) {
                    s.export_waypoints();
                } else if button == 0 && inside(import, mouse) {
                    s.import_waypoints();
                } else {
                    match s.list_row_at(mouse) {
                        // A group's heading shows or hides all of its waypoints.
                        Some(ListRow::Group(group)) => {
                            if button == 0 && !group.is_empty() && !s.hidden_groups.remove(&group) {
                                s.hidden_groups.insert(group);
                            }
                            s.wp_dirty = true;
                        }
                        Some(ListRow::Item(i)) => {
                            if button == 0 {
                                let w = &s.waypoints[i];
                                s.cx = f64::from(w.x) + 0.5;
                                s.cz = f64::from(w.z) + 0.5;
                                s.zoom_anchor = None;
                            } else if button == 1 {
                                open_menu(game, s, MenuKind::Waypoint(i), mouse);
                            }
                        }
                        None => {}
                    }
                }
            }
            return;
        }
        s.list_focus = false;
        // A click on the cave panel: a row does its thing, the rest is the panel's.
        if s.cave_panel && inside(s.cave_rect(), mouse) {
            s.ui_press = true;
            if button == 0
                && let Some(row) = s.cave_row_at(mouse)
            {
                if row == 3 {
                    s.cave_slider_drag = true;
                    s.cave_slider_set(mouse[0]);
                } else if row == 2 && inside(s.cave_auto_rect(), mouse) {
                    s.top_y = None;
                    s.top_input = None;
                    s.map.invalidate();
                } else {
                    s.cave_action(row);
                }
            }
            return;
        }
        match button {
            0 => {
                if s.slider_hit(mouse) {
                    s.slider_drag = true;
                    s.ui_press = true;
                    s.slider_set(mouse[1]);
                } else if let Some(tool) = s.tool_at(mouse) {
                    s.ui_press = true;
                    s.tool_action(tool);
                }
            }
            1 => {
                let kind = match s.waypoint_at(mouse) {
                    Some(i) => MenuKind::Waypoint(i),
                    None => {
                        let (x, z) = s.to_world(mouse);
                        MenuKind::Ground {
                            x: x.floor() as i32,
                            z: z.floor() as i32,
                        }
                    }
                };
                open_menu(game, s, kind, mouse);
            }
            _ => {}
        }
    });
}

/// A mouse button came up: the map takes it.
pub fn released(game: &Game<'_>, canceled: &mut bool) {
    if ours(game) {
        *canceled = true;
        with(|s| {
            s.slider_drag = false;
            s.cave_slider_drag = false;
            s.ui_press = false;
            // Let go while still moving: the map goes on a little.
            let moving = s.drag_at.is_some_and(|t| t.elapsed() < Duration::from_millis(90));
            let speed = s.drag_vel.0.hypot(s.drag_vel.1);
            if moving && speed >= 60.0 && s.entering.is_none() {
                s.glide = (s.drag_vel.0.clamp(-3000.0, 3000.0), s.drag_vel.1.clamp(-3000.0, 3000.0));
                s.glide_at = Instant::now();
            }
            s.drag_at = None;
            s.drag_vel = (0.0, 0.0);
        });
    }
}

/// The mouse moved with a button down: the left one drags the map or the slider.
pub fn dragged(
    game: &Game<'_>,
    screen: &NativeScreenView<'_>,
    button: i32,
    delta: [f32; 2],
    canceled: &mut bool,
) {
    if !ours(game) {
        return;
    }
    *canceled = true;
    with(|s| {
        s.mouse = screen.mouse;
        if s.slider_drag {
            s.slider_set(screen.mouse[1]);
        } else if s.cave_slider_drag {
            s.cave_slider_set(screen.mouse[0]);
        } else if button == 0 && !s.ui_press && s.entering.is_none() {
            let bpp = f64::from(s.bpp);
            s.cx -= f64::from(delta[0]) * bpp;
            s.cz -= f64::from(delta[1]) * bpp;
            s.zoom_anchor = None;
            s.glide = (0.0, 0.0);
            let now = Instant::now();
            if let Some(before) = s.drag_at {
                let dt = now.duration_since(before).as_secs_f64();
                if dt > 0.0005 {
                    let (vx, vy) = (f64::from(delta[0]) / dt, f64::from(delta[1]) / dt);
                    s.drag_vel = (s.drag_vel.0 * 0.5 + vx * 0.5, s.drag_vel.1 * 0.5 + vy * 0.5);
                }
            }
            s.drag_at = Some(now);
        }
    });
}

/// The wheel zooms, towards the pointer.
pub fn scrolled(
    game: &Game<'_>,
    screen: &NativeScreenView<'_>,
    scroll: [f64; 2],
    canceled: &mut bool,
) {
    if !ours(game) {
        return;
    }
    *canceled = true;
    with(|s| {
        s.mouse = screen.mouse;
        if s.list_open && inside(s.list_rect(), screen.mouse) {
            s.list_scroll = (s.list_scroll - scroll[1] as f32 * 28.0).max(0.0);
        } else if s.entering.is_none() && s.menu.is_none() {
            s.zoom_at(scroll[1] as f32, screen.mouse);
        }
    });
}

/// A character typed on the map screen: part of a waypoint's name.
pub fn typed(game: &Game<'_>, c: char, canceled: &mut bool) {
    if !ours(game) {
        return;
    }
    *canceled = true;
    with(|s| {
        if !s.cave_panel {
            s.top_input = None;
        }
        if let Some(text) = &mut s.top_input {
            if (c.is_ascii_digit() || (c == '-' && text.is_empty())) && text.len() < 6 {
                text.push(c);
            }
            return;
        }
        if let Some(skipped) = s.skip_char.take()
            && skipped.eq_ignore_ascii_case(&c)
        {
            return;
        }
        if let Some(text) = &mut s.goto {
            if (c.is_ascii_digit() || matches!(c, '-' | ' ' | ',')) && text.len() < 24 {
                text.push(c);
            }
            return;
        }
        if s.list_open && s.list_focus {
            if !c.is_control() && s.list_query.chars().count() < 24 {
                s.list_query.push(c);
                s.list_scroll = 0.0;
            }
            return;
        }
        if let Some((name, _, _)) = &mut s.entering
            && !c.is_control()
            && name.chars().count() < 32
        {
            name.push(c);
        }
    });
}

/// The map's keys as the player has them: what they chose, else the default.
struct Keys {
    open: String,
    waypoint: String,
    caves: String,
    grid: String,
    center: String,
    zoom_in: String,
    zoom_out: String,
    waypoints: String,
    goto: String,
    list: String,
    wp_edit: String,
    wp_teleport: String,
    wp_hide: String,
    wp_delete: String,
}

fn keys(game: &Game<'_>) -> Keys {
    let get = |name: &str| {
        let default = KEYS.iter().find(|k| k.0 == name).map_or("", |k| k.2);
        game.native_mod_key(ID, name, default)
    };
    Keys {
        open: get("open"),
        waypoint: get("waypoint"),
        caves: get("caves"),
        grid: get("grid"),
        center: get("center"),
        zoom_in: get("zoom_in"),
        zoom_out: get("zoom_out"),
        waypoints: get("waypoints"),
        goto: get("goto"),
        list: get("list"),
        wp_edit: get("wp_edit"),
        wp_teleport: get("wp_teleport"),
        wp_hide: get("wp_hide"),
        wp_delete: get("wp_delete"),
    }
}

/// The letter a key types (`key.keyboard.b` types b), if it is one.
fn letter_of(key: &str) -> Option<char> {
    let mut chars = key.strip_prefix("key.keyboard.")?.chars();
    let letter = chars.next()?;
    chars.next().is_none().then_some(letter)
}

/// A click on a button of the mod's page: a Reset of Key Binds is 1000 + the
/// key's place, and puts the key's default back.
pub fn settings_click(game: &mut Game<'_>, mod_id: &str, button: usize) {
    if mod_id != ID {
        return;
    }
    if button < 1000 {
        // The two after the switches take the minimap's corner and size round.
        if button == SETTINGS.len() || button == SETTINGS.len() + 1 {
            let mut settings = with(|s| s.settings);
            let (key, value) = if button == SETTINGS.len() {
                settings.corner = (settings.corner + 1) % MINI_CORNERS.len() as u8;
                ("minimap_corner", settings.corner)
            } else {
                settings.size = (settings.size + 1) % MINI_SIZES.len() as u8;
                ("minimap_size", settings.size)
            };
            game.native_set_setting(ID, key, &value.to_string());
            with(|s| {
                s.settings = settings;
                s.settings_loaded = true;
                s.slot = 0;
            });
            return;
        }
        // A switch of the settings page.
        let mut settings = with(|s| s.settings);
        if let Some((key, _)) = SETTINGS.get(button) {
            let on = !settings.get(button);
            settings.set(button, on);
            game.native_set_setting(ID, key, if on { "true" } else { "false" });
            with(|s| {
                s.settings = settings;
                s.settings_loaded = true;
            });
        }
        return;
    }
    if let Some(key) = KEYS.get(button - 1000) {
        game.native_set_mod_key(ID, key.0, None);
    }
}

/// The labels of the settings page's switches.
pub fn settings_page(game: &mut Game<'_>, mod_id: &str, buttons: &mut Vec<String>) {
    if mod_id != ID {
        return;
    }
    let ru = game.settings.language.starts_with("ru");
    let settings = Settings::read(game);
    for (i, (_, label)) in SETTINGS.iter().enumerate() {
        let name = if ru { ru_text(label).unwrap_or(label) } else { label };
        let state = match (settings.get(i), ru) {
            (true, true) => "ВКЛ",
            (false, true) => "ВЫКЛ",
            (true, false) => "ON",
            (false, false) => "OFF",
        };
        buttons.push(format!("{name}: {state}"));
    }
    let corner = MINI_CORNERS[usize::from(settings.corner).min(MINI_CORNERS.len() - 1)];
    let size = MINI_SIZE_NAMES[usize::from(settings.size).min(MINI_SIZE_NAMES.len() - 1)];
    let word = |text: &str| -> String {
        if ru { ru_text(text).unwrap_or(text).to_owned() } else { text.to_owned() }
    };
    buttons.push(format!("{}: {}", word("Minimap corner"), word(corner)));
    buttons.push(format!("{}: {}", word("Minimap size"), word(size)));
}

/// How the mod is listed in Options > Mods: title, description and keys, in the
/// game's language.
pub fn listing(game: &Game<'_>) -> (String, String, Vec<[String; 3]>) {
    let ru = game.settings.language.starts_with("ru");
    let title = if ru {
        format!("Карта мира {VERSION}")
    } else {
        format!("{NAME} {VERSION}")
    };
    let description = if ru {
        "Полноэкранная карта всего, что вы исследовали, с запоминанием между сеансами: пещерные слои, метки, телепорт, экспорт в PNG."
    } else {
        DESCRIPTION
    };
    let keys = KEYS
        .iter()
        .map(|k| {
            let label = if ru { ru_text(k.1).unwrap_or(k.1) } else { k.1 };
            [k.0.to_owned(), label.to_owned(), k.2.to_owned()]
        })
        .collect();
    (title, description.to_owned(), keys)
}

/// A key went down on the map screen.
pub fn screen_key(game: &Game<'_>, key: &str, canceled: &mut bool) {
    if !ours(game) {
        return;
    }
    let kb = keys(game);
    with(|s| {
        if !s.cave_panel {
            s.top_input = None;
        }
        if s.top_input.is_some() {
            *canceled = true;
            match key {
                "key.keyboard.enter" | "key.keyboard.keypad.enter" => s.commit_top_y(),
                "key.keyboard.escape" => s.top_input = None,
                "key.keyboard.backspace" => {
                    if let Some(text) = &mut s.top_input {
                        text.pop();
                    }
                }
                _ => {}
            }
            return;
        }
        if s.goto.is_some() {
            *canceled = true;
            match key {
                "key.keyboard.enter" | "key.keyboard.keypad.enter" => s.commit_goto(),
                "key.keyboard.escape" => s.goto = None,
                "key.keyboard.backspace" => {
                    if let Some(text) = &mut s.goto {
                        text.pop();
                    }
                }
                _ => {}
            }
            return;
        }
        if s.list_open && s.list_focus {
            *canceled = true;
            match key {
                "key.keyboard.backspace" => {
                    s.list_query.pop();
                    s.list_scroll = 0.0;
                }
                "key.keyboard.escape" | "key.keyboard.enter" | "key.keyboard.keypad.enter" => {
                    s.list_focus = false;
                }
                _ => {}
            }
            return;
        }
        if s.entering.is_some() {
            *canceled = true;
            s.typing_key(key);
            return;
        }
        if s.confirm_share.is_some() {
            *canceled = true;
            match key {
                "key.keyboard.enter" | "key.keyboard.keypad.enter" => s.answer_share(true),
                "key.keyboard.escape" => s.answer_share(false),
                _ => {}
            }
            return;
        }
        // The hotkeys of a waypoint's menu, and of the waypoint under the pointer.
        let target = match s.menu {
            Some((MenuKind::Waypoint(i), _)) => Some(i),
            Some(_) => None,
            None => s.waypoint_at(s.mouse),
        };
        if let Some(i) = target.filter(|&i| i < s.waypoints.len()) {
            let row = if key == kb.wp_edit {
                Some(0)
            } else if key == kb.wp_teleport {
                Some(1)
            } else if key == kb.wp_hide {
                Some(5)
            } else if key == kb.wp_delete {
                Some(7)
            } else {
                None
            };
            if let Some(row) = row {
                *canceled = true;
                s.menu = None;
                s.menu_action(MenuKind::Waypoint(i), row);
                return;
            }
        }
        // Escape closes the cave panel before it closes the screen.
        if s.cave_panel && key == "key.keyboard.escape" {
            s.cave_panel = false;
            *canceled = true;
            return;
        }
        // Escape closes an open menu before it closes the screen.
        if s.menu.is_some() && key == "key.keyboard.escape" {
            s.menu = None;
            *canceled = true;
            return;
        }
        if key == kb.open {
            // `key` closes the screen, the game's own hook sees it.
            return;
        }
        let step = 32.0 * f64::from(s.bpp);
        let centre = [s.gui.0 / 2.0, s.gui.1 / 2.0];
        *canceled = true;
        match key {
            "key.keyboard.left" | "key.keyboard.a" => s.cx -= step,
            "key.keyboard.right" | "key.keyboard.d" => s.cx += step,
            "key.keyboard.up" | "key.keyboard.w" => s.cz -= step,
            "key.keyboard.down" | "key.keyboard.s" => s.cz += step,
            k if k == kb.zoom_in || k == "key.keyboard.keypad.add" => s.zoom_at(1.0, centre),
            k if k == kb.zoom_out || k == "key.keyboard.keypad.subtract" => {
                s.zoom_at(-1.0, centre);
            }
            k if k == kb.center || k == "key.keyboard.home" => {
                s.cx = s.player.0;
                s.cz = s.player.2;
            }
            k if k == kb.waypoints => {
                s.show_waypoints = !s.show_waypoints;
                let text = if s.show_waypoints {
                    "Waypoints shown"
                } else {
                    "Waypoints hidden"
                };
                s.notice = Some((text.to_owned(), Instant::now()));
            }
            k if k == kb.goto => {
                s.skip_char = letter_of(k);
                s.top_input = None;
                s.goto = Some(String::new());
            }
            k if k == kb.list => {
                s.list_open = !s.list_open;
                s.list_focus = false;
            }
            k if k == kb.grid => s.grid = !s.grid,
            k if k == kb.caves => s.cave_panel = !s.cave_panel,
            "key.keyboard.page_up" | "key.keyboard.page_down" => {
                // The cave map's top: up or down 16 blocks.
                if s.view_layer != SURFACE && s.view_layer != FULL {
                    let step = if key.ends_with("up") { 16 } else { -16 };
                    let top = s.top_y.unwrap_or(layer_top(s.view_layer)) + step;
                    s.top_y = Some(top.clamp(TOP_MIN, TOP_MAX));
                    s.map.invalidate();
                }
            }
            "key.keyboard.end" => {
                s.top_y = None;
                s.map.invalidate();
            }
            k if k == kb.waypoint => {
                s.skip_char = letter_of(k);
                let at = s.mouse;
                s.begin_waypoint(at);
            }
            // Escape and the rest are the game's.
            _ => *canceled = false,
        }
    });
}

// -- Drawing -------------------------------------------------------------------

/// A waypoint's marker: a diamond, a square or a pennant, with a dark outline.
/// How much coarser than the screen the map's image is to be drawn: 1 while a sharp image
/// (`w` x `h`, with its margin) costs under 16 ms, else just enough that it does. `now` is
/// kept unless the need has moved well away from it, so the image does not change size back and
/// forth.
fn quality_for(px_cost: f32, w: f32, h: f32, now: f32) -> f32 {
    if px_cost <= 0.0 {
        return now;
    }
    let sharp_ms = px_cost * w * h * 1.8;
    let needed = (sharp_ms / 16.0).sqrt().clamp(0.5, 3.0);
    let target = if needed <= 1.15 { 1.0 } else { (needed * 4.0).ceil() / 4.0 };
    if target > now || needed < now * 0.8 { target } else { now }
}

/// A letter, white with a dark edge: it reads on any colour.
fn outlined(game: &Game<'_>, batch: &mut HudBatch, text: &str, x: f32, y: f32) {
    let left = (x - width_of(game, text) / 2.0).round();
    let top = y.round();
    for (dx, dy) in [(-1.0, 0.0), (1.0, 0.0), (0.0, -1.0), (0.0, 1.0), (-1.0, 1.0), (1.0, 1.0)] {
        say(game, batch, text, [left + dx, top + dy], 0x0A_0C12, false);
    }
    say(game, batch, text, [left, top], 0xFFFFFF, false);
}

/// A waypoint on the map: its colour in the shape it has (a diamond, a square or a pin), big
/// enough for its first letter, lit with a white edge when the pointer is on it.
fn badge(game: &Game<'_>, batch: &mut HudBatch, x: f32, y: f32, w: &Waypoint, hot: bool) {
    let fill = 0xFF00_0000 | w.color;
    // The shape: its outline and then its colour, from the outside in.
    let layers: [(f32, u32); 3] = if hot {
        [(2.6, 0xFFFF_FFFF), (1.3, 0xFF0A_0C12), (0.0, fill)]
    } else {
        [(1.3, 0xFF0A_0C12), (0.0, fill), (0.0, fill)]
    };
    for (grow, color) in layers {
        match w.shape {
            1 => {
                let h = 6.5 + grow;
                game.native_fill(batch, [x - h, y - h, h * 2.0, h * 2.0], color);
            }
            2 => {
                // A pin: a square with a point under it.
                let h = 6.5 + grow;
                game.native_fill(batch, [x - h, y - h - 1.0, h * 2.0, h * 2.0], color);
                game.native_quad(
                    batch,
                    [
                        [x - 4.0 - grow, y + h - 1.0],
                        [x + 4.0 + grow, y + h - 1.0],
                        [x + 4.0 + grow, y + h - 1.0],
                        [x, y + h + 4.0 + grow],
                    ],
                    color,
                );
            }
            _ => {
                let r = 9.0 + grow;
                game.native_quad(
                    batch,
                    [[x, y - r], [x + r, y], [x, y + r], [x - r, y]],
                    color,
                );
            }
        }
    }
    let initial: String = w.name.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
    let dy = if w.shape == 2 { -4.5 } else { -3.5 };
    outlined(game, batch, &initial, x, y + dy);
}

fn marker(game: &Game<'_>, batch: &mut HudBatch, x: f32, y: f32, r: f32, rgb: u32, shape: u8) {
    let (outline, fill) = match shape {
        1 => (
            [
                [x - r - 1.0, y - r - 1.0],
                [x + r + 1.0, y - r - 1.0],
                [x + r + 1.0, y + r + 1.0],
                [x - r - 1.0, y + r + 1.0],
            ],
            [
                [x - r, y - r],
                [x + r, y - r],
                [x + r, y + r],
                [x - r, y + r],
            ],
        ),
        2 => (
            [
                [x, y - r - 2.0],
                [x + r + 2.0, y + r + 1.0],
                [x + r + 2.0, y + r + 1.0],
                [x - r - 2.0, y + r + 1.0],
            ],
            [
                [x, y - r],
                [x + r, y + r],
                [x + r, y + r],
                [x - r, y + r],
            ],
        ),
        _ => (
            [
                [x, y - r - 1.0],
                [x + r + 1.0, y],
                [x, y + r + 1.0],
                [x - r - 1.0, y],
            ],
            [[x, y - r], [x + r, y], [x, y + r], [x - r, y]],
        ),
    };
    game.native_quad(batch, outline, 0xFF00_0000);
    game.native_quad(batch, fill, 0xFF00_0000 | rgb);
}

/// A waypoint in the world: a square of its colour with its first letter, its name and
/// distance under it on a dark ground.
fn waypoint_icon(game: &Game<'_>, batch: &mut HudBatch, x: f32, y: f32, w: &Waypoint, label: &str) {
    game.native_fill(batch, [x - 7.0, y - 7.0, 14.0, 14.0], 0xC000_0000);
    game.native_fill(batch, [x - 6.0, y - 6.0, 12.0, 12.0], 0xFF00_0000 | w.color);
    let initial: String = w.name.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
    outlined(game, batch, &initial, x, y - 3.5);
    let lw = width_of(game, label);
    game.native_fill(batch, [(x - lw / 2.0 - 3.0).round(), y + 9.0, lw + 6.0, 12.0], 0xA000_0000);
    centered_text(game, batch, label, x, y + 11.5, 0xFFFFFF);
}

fn centered_text(game: &Game<'_>, batch: &mut HudBatch, text: &str, x: f32, y: f32, rgb: u32) {
    let w = width_of(game, text);
    say(game, batch, text, [(x - w / 2.0).round(), y.round()], rgb, true);
}

/// A panel with an accent bar on its left edge.
fn panel(game: &Game<'_>, batch: &mut HudBatch, rect: [f32; 4]) {
    game.native_fill(batch, rect, PANEL);
    game.native_fill(batch, [rect[0], rect[1], 2.0, rect[3]], 0xFF00_0000 | ACCENT);
}

/// A button: a body with cut corners, a light top edge and a shade below; lit when
/// the pointer is on it, tinted when it is on, filled with the accent when primary.
fn button(game: &Game<'_>, batch: &mut HudBatch, r: [f32; 4], hot: bool, on: bool, primary: bool) {
    let [x, y, w, h] = r;
    let (back, edge) = if primary {
        (
            if hot { 0xFF2A_9A91 } else { 0xFF1F_7A73 },
            0xFF00_0000 | ACCENT,
        )
    } else if on {
        (
            if hot { 0xFF1C_4448 } else { 0xFF16_363A },
            0xFF00_0000 | ACCENT,
        )
    } else if hot {
        (0xFF26_3447, 0xFF3E_5570)
    } else {
        (0xFF1B_2330, EDGE)
    };
    game.native_fill(batch, [x + 1.0, y + 1.0, w - 2.0, h - 2.0], back);
    game.native_fill(batch, [x + 1.0, y, w - 2.0, 1.0], edge);
    game.native_fill(batch, [x + 1.0, y + h - 1.0, w - 2.0, 1.0], edge);
    game.native_fill(batch, [x, y + 1.0, 1.0, h - 2.0], edge);
    game.native_fill(batch, [x + w - 1.0, y + 1.0, 1.0, h - 2.0], edge);
    game.native_fill(batch, [x + 2.0, y + 1.0, w - 4.0, 1.0], 0x20FF_FFFF);
    game.native_fill(batch, [x + 2.0, y + h - 2.0, w - 4.0, 1.0], 0x4000_0000);
}

/// A text field: a dark box with an edge that turns to the accent while typing.
fn field_box(game: &Game<'_>, batch: &mut HudBatch, r: [f32; 4], active: bool) {
    let [x, y, w, h] = r;
    let edge = if active { 0xFF00_0000 | ACCENT } else { EDGE };
    game.native_fill(batch, [x + 1.0, y + 1.0, w - 2.0, h - 2.0], 0xFF0B_0E14);
    game.native_fill(batch, [x + 1.0, y, w - 2.0, 1.0], edge);
    game.native_fill(batch, [x + 1.0, y + h - 1.0, w - 2.0, 1.0], edge);
    game.native_fill(batch, [x, y + 1.0, 1.0, h - 2.0], edge);
    game.native_fill(batch, [x + w - 1.0, y + 1.0, 1.0, h - 2.0], edge);
}

/// A toolbar icon centred on (`cx`, `cy`), by the tool's index: minus, plus, a
/// crosshair, a grid, a cave mouth, a pin and a tray with an arrow.
fn icon(game: &Game<'_>, batch: &mut HudBatch, kind: usize, cx: f32, cy: f32, rgb: u32) {
    let argb = 0xFF00_0000 | rgb;
    let bar = |b: &mut HudBatch, x: f32, y: f32, w: f32, h: f32| {
        game.native_fill(b, [cx + x, cy + y, w, h], argb);
    };
    match kind {
        0 => bar(batch, -4.0, -1.0, 8.0, 2.0),
        1 => {
            bar(batch, -4.0, -1.0, 8.0, 2.0);
            bar(batch, -1.0, -4.0, 2.0, 8.0);
        }
        2 => {
            bar(batch, -1.0, -5.0, 2.0, 3.0);
            bar(batch, -1.0, 2.0, 2.0, 3.0);
            bar(batch, -5.0, -1.0, 3.0, 2.0);
            bar(batch, 2.0, -1.0, 3.0, 2.0);
            bar(batch, -1.0, -1.0, 2.0, 2.0);
        }
        3 => {
            bar(batch, -4.0, -2.0, 9.0, 1.0);
            bar(batch, -4.0, 1.0, 9.0, 1.0);
            bar(batch, -2.0, -4.0, 1.0, 9.0);
            bar(batch, 1.0, -4.0, 1.0, 9.0);
        }
        4 => {
            bar(batch, -3.0, -4.0, 7.0, 1.0);
            bar(batch, -4.0, -3.0, 1.0, 7.0);
            bar(batch, 4.0, -3.0, 1.0, 7.0);
            bar(batch, -4.0, 4.0, 9.0, 1.0);
            bar(batch, -1.0, 0.0, 3.0, 4.0);
        }
        5 => {
            game.native_quad(
                batch,
                [[cx, cy - 5.0], [cx + 4.0, cy], [cx, cy + 5.0], [cx - 4.0, cy]],
                argb,
            );
        }
        6 => {
            bar(batch, -1.0, -5.0, 2.0, 5.0);
            game.native_quad(
                batch,
                [[cx - 4.0, cy], [cx + 4.0, cy], [cx, cy + 4.0], [cx, cy + 4.0]],
                argb,
            );
            bar(batch, -5.0, 5.0, 11.0, 1.0);
        }
        7 => {
            bar(batch, -4.0, -4.0, 9.0, 1.0);
            bar(batch, -4.0, 4.0, 9.0, 1.0);
            bar(batch, -4.0, -4.0, 1.0, 9.0);
            bar(batch, 4.0, -4.0, 1.0, 9.0);
            bar(batch, -4.0, 0.0, 9.0, 1.0);
            bar(batch, 0.0, -4.0, 1.0, 9.0);
        }
        _ => {
            bar(batch, -4.0, -3.0, 9.0, 1.0);
            bar(batch, -4.0, 0.0, 9.0, 1.0);
            bar(batch, -4.0, 3.0, 9.0, 1.0);
        }
    }
}

/// Corner brackets round a rectangle.
fn brackets(game: &Game<'_>, batch: &mut HudBatch, rect: [f32; 4], argb: u32) {
    let [x, y, w, h] = rect;
    let n = (w / 3.0).clamp(2.0, 7.0);
    for (px, py, dx, dy) in [
        (x, y, 1.0, 1.0),
        (x + w, y, -1.0, 1.0),
        (x, y + h, 1.0, -1.0),
        (x + w, y + h, -1.0, -1.0),
    ] {
        let hx = if dx > 0.0 { px } else { px - n };
        let vy = if dy > 0.0 { py } else { py - n };
        let hy = if dy > 0.0 { py } else { py - 1.0 };
        let vx = if dx > 0.0 { px } else { px - 1.0 };
        game.native_fill(batch, [hx, hy, n, 1.0], argb);
        game.native_fill(batch, [vx, vy, 1.0, n], argb);
    }
}

/// The map screen, over the game.
pub fn draw(game: &Game<'_>, batch: &mut HudBatch, screen: &NativeScreenView<'_>) {
    if !ours(game) {
        return;
    }
    let (gw, gh) = (batch.width, batch.height);
    with(|s| {
        s.gui = (gw, gh);
        s.mouse = screen.mouse;
        let mouse = screen.mouse;
        game.native_fill(batch, [0.0, 0.0, gw, gh], 0xFF0C_0C10);
        let (iw, ih, scale) = s.img;
        if let Some((ix, iz, ibpp, _, _)) = s.img_view
            && iw > 0
            && ih > 0
        {
            // The part of the image that is the screen: where the view is in it.
            let sx =
                ((s.cx - ix) / f64::from(ibpp)) as f32 + iw as f32 / 2.0 - gw / (2.0 * scale);
            let sy =
                ((s.cz - iz) / f64::from(ibpp)) as f32 + ih as f32 / 2.0 - gh / (2.0 * scale);
            let _ = game.native_sprite(
                batch,
                MOD_IMAGE_SPRITE,
                [sx, sy, gw / scale, gh / scale],
                [0.0, 0.0, gw, gh],
            );
        }
        let free = s.menu.is_none() && s.entering.is_none();
        let chunk_px = 16.0 / s.bpp;

        // The chunk grid.
        if s.grid && chunk_px >= 8.0 {
            let (x0, z0) = s.to_world([0.0, 0.0]);
            let (x1, z1) = s.to_world([gw, gh]);
            for c in ((x0.floor() as i32) >> 4)..=((x1.floor() as i32) >> 4) + 1 {
                let (x, _) = s.to_screen(f64::from(c * 16), 0.0);
                game.native_fill(batch, [x.round(), 0.0, 1.0, gh], 0x30FF_FFFF);
            }
            for c in ((z0.floor() as i32) >> 4)..=((z1.floor() as i32) >> 4) + 1 {
                let (_, y) = s.to_screen(0.0, f64::from(c * 16));
                game.native_fill(batch, [0.0, y.round(), gw, 1.0], 0x30FF_FFFF);
            }
        }

        // The chunk under the pointer.
        let (mx, mz) = s.to_world(mouse);
        let (bx, bz) = (mx.floor() as i32, mz.floor() as i32);
        if free && chunk_px >= 5.0 && mouse[1] < gh - TOOL_H - 8.0 && s.waypoint_at(mouse).is_none() {
            let (x, y) = s.to_screen(f64::from((bx >> 4) * 16), f64::from((bz >> 4) * 16));
            brackets(game, batch, [x, y, chunk_px, chunk_px], 0xE000_0000 | ACCENT);
        }

        // Chunks the game has loaded that are not on the map yet, pulsing.
        if chunk_px >= 3.0 && !s.viewing_other() {
            let pulse = ((s.born.elapsed().as_secs_f32() * 4.0).sin() * 0.5 + 0.5) * 48.0;
            let alpha = 0x24 + pulse as u32;
            for (cx, cz) in &s.waiting {
                let (x, y) = s.to_screen(f64::from(cx * 16), f64::from(cz * 16));
                if x < -chunk_px || y < -chunk_px || x > gw || y > gh {
                    continue;
                }
                game.native_fill(batch, [x, y, chunk_px, chunk_px], (alpha << 24) | ACCENT);
            }
        }

        // Where the player has been.
        let count = if s.settings.trail && !s.viewing_other() { s.trail.len() } else { 0 };
        for (i, (tx, tz)) in s.trail.iter().enumerate() {
            let (x, y) = s.to_screen(*tx, *tz);
            if x < 0.0 || y < 0.0 || x > gw || y > gh {
                continue;
            }
            let alpha = 0x30 + (i * 0x70 / count.max(1)) as u32;
            game.native_fill(batch, [x - 1.0, y - 1.0, 2.0, 2.0], (alpha << 24) | ACCENT);
        }

        // Waypoints.
        let over_panel = (s.cave_panel && inside(s.cave_rect(), mouse))
            || (s.list_open && inside(s.list_rect(), mouse))
            || mouse[1] >= gh - TOOL_H - 12.0;
        let hovered = if free && !over_panel { s.waypoint_at(mouse) } else { None };
        for (n, w) in s
            .waypoints
            .iter()
            .enumerate()
            .filter(|(_, w)| s.show_waypoints && w.dim == s.view_dim_id() && s.group_visible(w))
        {
            let (x, y) = s.to_screen(f64::from(w.x) + 0.5, f64::from(w.z) + 0.5);
            if x < -20.0 || y < -20.0 || x > gw + 20.0 || y > gh + 20.0 {
                continue;
            }
            badge(game, batch, x, y, w, hovered == Some(n));
        }

        // The player: a chevron where they stand, pointing where they face.
        let (px, py, pz, yaw) = s.player;
        let (x, y) = s.to_screen(px, pz);
        if x > -10.0 && y > -10.0 && x < gw + 10.0 && y < gh + 10.0 && !s.viewing_other() {
            let (sin, cos) = f64::from(yaw).to_radians().sin_cos();
            let f = [-sin as f32, cos as f32];
            let n = [-f[1], f[0]];
            let at = |a: f32, b: f32| [x + f[0] * a + n[0] * b, y + f[1] * a + n[1] * b];
            game.native_quad(
                batch,
                [at(10.0, 0.0), at(-6.0, 7.0), at(-2.0, 0.0), at(-6.0, -7.0)],
                0xFF00_0000 | ACCENT,
            );
            game.native_quad(
                batch,
                [at(7.0, 0.0), at(-4.0, 4.5), at(-1.0, 0.0), at(-4.0, -4.5)],
                0xFFFF_FFFF,
            );
        }

        // Other players, where the game knows them.
        if s.settings.players && !s.viewing_other() {
            for (name, position) in &s.players {
                if (position[0] - px).hypot(position[2] - pz) < 1.5 {
                    continue;
                }
                let (x, y) = s.to_screen(position[0], position[2]);
                if x < -20.0 || y < -20.0 || x > gw + 20.0 || y > gh + 20.0 {
                    continue;
                }
                marker(game, batch, x, y, 3.0, 0xFFFFFF, 0);
                centered_text(game, batch, name, x, y - 13.0, 0xFFFFFF);
            }
        }

        // Top left: what is under the pointer.
        let height = s
            .ground_y(bx, bz)
            .map_or_else(|| "?".to_owned(), |y| y.to_string());
        let line1 = format!("X {bx}   Y {height}   Z {bz}");
        let dist = ((f64::from(bx) - px).powi(2) + (f64::from(bz) - pz).powi(2)).sqrt();
        let line2 = if ru() {
            format!("чанк {}, {}   {} м от вас", bx >> 4, bz >> 4, dist.round() as i64)
        } else {
            format!("chunk {}, {}   {} m from you", bx >> 4, bz >> 4, dist.round() as i64)
        };
        let line3 = biome_line(game, s, bx, bz);
        let w1 = game
            .native_text_width(&line1)
            .max(width_of(game, &line2))
            .max(width_of(game, &line3));
        let rows1 = if line3.is_empty() { 24.0 } else { 34.0 };
        panel(game, batch, [6.0, 6.0, w1 + 14.0, rows1]);
        say(game, batch, &line1, [12.0, 9.0], TEXT, true);
        say(game, batch, &line2, [12.0, 19.0], MUTED, true);
        if !line3.is_empty() {
            say(game, batch, &line3, [12.0, 29.0], ACCENT, true);
        }

        // Top right: north.
        let (cx, cy) = (gw - 22.0, 20.0);
        game.native_fill(batch, [cx - 15.0, cy - 14.0, 30.0, 28.0], PANEL);
        game.native_quad(
            batch,
            [[cx, cy - 11.0], [cx + 5.0, cy - 2.0], [cx, cy - 4.0], [cx - 5.0, cy - 2.0]],
            0xFF00_0000 | ACCENT,
        );
        centered_text(game, batch, "N", cx, cy + 2.0, TEXT);

        // The zoom slider.
        let [sx, sy, sw, sh] = s.slider();
        game.native_fill(batch, [sx - 3.0, sy - 12.0, sw + 6.0, sh + 24.0], PANEL);
        game.native_fill(batch, [sx + sw / 2.0 - 1.0, sy, 2.0, sh], EDGE);
        let hy = sy + s.slider_value() * sh;
        game.native_fill(batch, [sx - 1.0, hy - 3.0, sw + 2.0, 6.0], 0xFF00_0000 | ACCENT);
        centered_text(game, batch, "+", sx + sw / 2.0, sy - 11.0, MUTED);
        centered_text(game, batch, "-", sx + sw / 2.0, sy + sh + 2.0, MUTED);

        // Bottom left: the map and the zoom.
        let layer_line = if s.shown_layer() == SURFACE {
            tr("surface").into_owned()
        } else if s.view_layer == FULL {
            tr("caves, every height").into_owned()
        } else if ru() {
            let hand = if s.top_y.is_some() { " (задано)" } else { "" };
            format!("пещеры, Y до {}{hand}", layer_top(s.view_layer))
        } else {
            let hand = if s.top_y.is_some() { " (set)" } else { "" };
            format!("caves, Y up to {}{hand}", layer_top(s.view_layer))
        };
        let explored = s.shown().explored_chunks(s.shown_layer());
        let line1 = if ru() {
            format!("{}   {} чанков", dim_label(s.view_dim_id()), explored)
        } else {
            format!("{}   {} chunks", dim_label(s.view_dim_id()), explored)
        };
        let line2 = format!("{:.2}x   {layer_line}", 1.0 / s.bpp);
        let loading = (!s.waiting.is_empty() && !s.viewing_other()).then(|| {
            if ru() {
                format!("Загрузка: {} чанков", s.waiting.len())
            } else {
                format!("Loading {} chunks", s.waiting.len())
            }
        });
        let mut w2 = width_of(game, &line1).max(width_of(game, &line2));
        if let Some(text) = &loading {
            w2 = w2.max(width_of(game, text));
        }
        let rows = if loading.is_some() { 36.0 } else { 24.0 };
        s.info_w = w2 + 14.0;
        let bottom = gh - TOOL_H - 12.0;
        panel(game, batch, [6.0, bottom - rows, w2 + 14.0, rows]);
        let top = bottom - rows + 3.0;
        say(game, batch, &line1, [12.0, top], TEXT, true);
        say(game, batch, &line2, [12.0, top + 10.0], MUTED, true);
        if let Some(text) = &loading {
            say(game, batch, text, [12.0, top + 20.0], ACCENT, true);
        }

        // The toolbar.
        let rects = s.toolbar();
        for (i, (rect, tool)) in rects.iter().zip(TOOLS).enumerate() {
            let on = (i == 3 && s.grid)
                || (i == 4 && (s.cave_panel || s.view_layer != SURFACE))
                || (i == 7 && s.viewing_other())
                || (i == 8 && s.list_open);
            let hot = free && inside(*rect, mouse);
            button(game, batch, *rect, hot, on, false);
            let color = if on { ACCENT } else { TEXT };
            let cy = rect[1] + rect[3] / 2.0;
            if tool.0.is_empty() || s.compact() {
                icon(game, batch, i, rect[0] + rect[2] / 2.0, cy, color);
            } else {
                icon(game, batch, i, rect[0] + 10.0, cy, color);
                let label = if i == 7 {
                    dim_label(s.view_dim_id())
                } else {
                    tool.0.to_owned()
                };
                say(game, batch, &label, [rect[0] + 19.0, cy - 3.5], color, true);
            }
        }

        // The waypoint list: the ones of the dimension shown, nearest first.
        if s.list_open {
            let r = s.list_rect();
            panel(game, batch, r);
            let items = s.list_rows();
            let title = format!("{} ({})", tr("Waypoints"), s.list_items().len());
            say(game, batch, &title, [r[0] + 8.0, r[1] + 4.0], TEXT, true);
            let sr = s.list_search_rect();
            field_box(game, batch, sr, s.list_focus);
            let empty = s.list_query.is_empty() && !s.list_focus;
            let query = if empty {
                tr("Search").into_owned()
            } else {
                format!("{}{}", s.list_query, if s.list_focus { "_" } else { "" })
            };
            say(game, batch, &query, [sr[0] + 4.0, sr[1] + 4.0], if empty { MUTED } else { TEXT }, true);
            let top = r[1] + 36.0;
            let height = r[3] - 38.0 - 20.0;
            let most = (items.len() as f32 * 14.0 - height).max(0.0);
            s.list_scroll = s.list_scroll.clamp(0.0, most);
            let (ox, oz) = if s.viewing_other() { (s.cx, s.cz) } else { (px, pz) };
            for (k, row) in items.iter().enumerate() {
                let y = top + k as f32 * 14.0 - s.list_scroll;
                if y < top || y + 13.0 > top + height {
                    continue;
                }
                if free && inside([r[0], y, r[2], 13.0], mouse) {
                    game.native_fill(batch, [r[0] + 2.0, y, r[2] - 2.0, 13.0], HOT);
                }
                let i = match row {
                    ListRow::Item(i) => *i,
                    ListRow::Group(group) => {
                        // A heading: a box that is filled when the group is shown.
                        let hidden = s.hidden_groups.contains(group);
                        let edge = if group.is_empty() { 0xFF30_3846 } else { 0xFF00_0000 | ACCENT };
                        game.native_fill(batch, [r[0] + 8.0, y + 3.0, 7.0, 7.0], edge);
                        if !hidden {
                            game.native_fill(batch, [r[0] + 9.0, y + 4.0, 5.0, 5.0], 0xFF10_141C | if group.is_empty() { 0x20_2830 } else { ACCENT });
                        } else {
                            game.native_fill(batch, [r[0] + 9.0, y + 4.0, 5.0, 5.0], 0xFF10_141C);
                        }
                        let name = if group.is_empty() { tr("No group").into_owned() } else { group.clone() };
                        let mut name = name;
                        while width_of(game, &name) > r[2] - 30.0 && name.chars().count() > 1 {
                            name.pop();
                        }
                        say(game, batch, &name, [r[0] + 19.0, y + 3.0], if hidden { MUTED } else { WARN }, true);
                        continue;
                    }
                };
                let w = &s.waypoints[i];
                let dimmed = !s.group_visible(w);
                game.native_fill(batch, [r[0] + 8.0, y + 3.0, 7.0, 7.0], 0xFF00_0000 | w.color);
                let far = (f64::from(w.x) - ox).hypot(f64::from(w.z) - oz);
                let far = if far >= 10_000.0 {
                    format!("{:.0}k", far / 1000.0)
                } else {
                    format!("{}", far.round() as i64)
                };
                let fw = width_of(game, &far);
                let mut name = w.name.clone();
                while width_of(game, &name) > r[2] - 30.0 - fw && name.chars().count() > 1 {
                    name.pop();
                }
                say(game, batch, &name, [r[0] + 19.0, y + 3.0], if dimmed { MUTED } else { TEXT }, true);
                say(game, batch, &far, [r[0] + r[2] - 6.0 - fw, y + 3.0], MUTED, true);
            }
            // Export and Import, under the rows.
            let (export, import) = s.list_buttons();
            for (rect, label) in [(export, "Export"), (import, "Import")] {
                button(game, batch, rect, free && inside(rect, mouse), false, false);
                centered_text(game, batch, label, rect[0] + rect[2] / 2.0, rect[1] + 3.0, TEXT);
            }
        }

        // The cave panel: mode, type of layers, and the top Y.
        if s.cave_panel {
            let r = s.cave_rect();
            panel(game, batch, r);
            say(game, batch, "Cave map", [r[0] + 8.0, r[1] + 4.0], TEXT, true);
            let hot = if free { s.cave_row_at(mouse) } else { None };
            let top_text = match (&s.top_input, s.top_y) {
                (Some(text), _) => format!("{text}_"),
                (None, Some(y)) => y.to_string(),
                (None, None) => format!("{} ({})", tr("auto"), s.effective_top()),
            };
            let rows = [
                format!(
                    "{}: {}",
                    tr("Cave mode"),
                    tr(["auto", "on", "off"][usize::from(s.cave_mode % 3)])
                ),
                format!(
                    "{}: {}",
                    tr("Type"),
                    tr(if s.cave_type == 1 { "full" } else { "layered" })
                ),
                format!("{}: {top_text}", tr("Top Y")),
            ];
            for i in 0..CAVE_ROWS {
                let y = r[1] + CAVE_HEAD + i as f32 * CAVE_ROW;
                let is_hot = hot == Some(i);
                // "Full" has no top: the Top Y rows wait.
                let dim = s.cave_type == 1 && i >= 2;
                let ink = if dim { MUTED } else { TEXT };
                match i {
                    0 | 1 => {
                        let rect = [r[0] + 8.0, y, r[2] - 16.0, CAVE_ROW - 2.0];
                        button(game, batch, rect, is_hot, false, false);
                        centered_text(game, batch, &rows[i], rect[0] + rect[2] / 2.0, y + 4.0, TEXT);
                    }
                    2 => {
                        // "Top Y" is a button, its value a field beside it, and Auto.
                        let editing = s.top_input.is_some();
                        let label = [r[0] + 8.0, y, 70.0, CAVE_ROW - 2.0];
                        button(game, batch, label, is_hot && !editing, false, false);
                        centered_text(game, batch, "Top Y", label[0] + label[2] / 2.0, y + 4.0, ink);
                        let auto = s.cave_auto_rect();
                        let field = [r[0] + 82.0, y, auto[0] - r[0] - 82.0 - 4.0, CAVE_ROW - 2.0];
                        field_box(game, batch, field, editing);
                        let color = if editing {
                            WARN
                        } else if s.top_y.is_some() {
                            ink
                        } else {
                            MUTED
                        };
                        say(game, batch, &top_text, [field[0] + 5.0, y + 4.0], color, true);
                        let auto_hot = free && inside(auto, mouse);
                        button(game, batch, auto, auto_hot, s.top_y.is_none(), false);
                        let auto_color = if s.top_y.is_none() { ACCENT } else { ink };
                        centered_text(game, batch, "Auto", auto[0] + auto[2] / 2.0, y + 4.0, auto_color);
                    }
                    _ => {
                        // The top, from the world's bottom to its top.
                        say(game, batch, "-64", [r[0] + 8.0, y + 4.0], MUTED, true);
                        let high = "319";
                        let hw = width_of(game, high);
                        say(game, batch, high, [r[0] + r[2] - 8.0 - hw, y + 4.0], MUTED, true);
                        let (tx, tw) = s.cave_track();
                        let v = s.effective_top().clamp(TOP_MIN, TOP_MAX);
                        let t = (v - TOP_MIN) as f32 / (TOP_MAX - TOP_MIN) as f32;
                        let hx = tx + t * tw;
                        let lit = if dim { EDGE } else { 0xFF00_0000 | ACCENT };
                        game.native_fill(batch, [tx, y + 6.0, tw, 3.0], EDGE);
                        game.native_fill(batch, [tx, y + 6.0, (hx - tx).max(0.0), 3.0], lit);
                        let grab = s.cave_slider_drag || (is_hot && !dim);
                        let body = if grab { 0xFFFF_FFFF } else { 0xFFC8_D2DC };
                        game.native_fill(batch, [hx - 4.0, y + 1.0, 8.0, 12.0], 0xFF00_0000);
                        game.native_fill(batch, [hx - 3.0, y + 2.0, 6.0, 10.0], body);
                        game.native_fill(batch, [hx - 1.0, y + 4.0, 2.0, 6.0], lit);
                    }
                }
            }
            let help = &CAVE_TEXT[hot.unwrap_or(0)];
            let base = r[1] + CAVE_HEAD + CAVE_ROWS as f32 * CAVE_ROW + 4.0;
            for (i, line) in help.iter().enumerate() {
                say(game, batch, line, [r[0] + 8.0, base + i as f32 * 10.0], MUTED, true);
            }
        }

        // The waypoint under the pointer: its name, its place, how far it is and what it is in,
        // and the hotkeys it answers to.
        if let Some(i) = hovered {
            let w = &s.waypoints[i];
            let kb = keys(game);
            let mut lines: Vec<(String, u32)> = vec![
                (w.name.clone(), w.color),
                (format!("X {}   Y {}   Z {}", w.x, w.y, w.z), TEXT),
            ];
            let mut facts: Vec<String> = Vec::new();
            if !s.viewing_other() {
                let far = ((f64::from(w.x) + 0.5 - px).powi(2) + (f64::from(w.z) + 0.5 - pz).powi(2)).sqrt();
                let unit = if ru() { "м" } else { "m" };
                facts.push(if far >= 10_000.0 {
                    format!("{:.1}k {unit}", far / 1000.0)
                } else {
                    format!("{} {unit}", far.round() as i64)
                });
            }
            if !w.group.is_empty() {
                facts.push(w.group.clone());
            }
            if !facts.is_empty() {
                lines.push((facts.join("  \u{b7}  "), MUTED));
            }
            let keys_line = [
                (&kb.wp_edit, pick("Edit", "Изменить")),
                (&kb.wp_teleport, pick("Teleport", "Телепорт")),
                (&kb.wp_hide, pick("Hide", "Скрыть")),
                (&kb.wp_delete, pick("Delete", "Удалить")),
            ]
            .iter()
            .filter(|(k, _)| !key_label(k).is_empty())
            .map(|(k, what)| format!("{} {what}", key_label(k)))
            .collect::<Vec<_>>()
            .join("   ");
            let width = lines
                .iter()
                .map(|l| width_of(game, &l.0))
                .chain(std::iter::once(width_of(game, &keys_line)))
                .fold(0.0_f32, f32::max)
                + 16.0;
            let height = (lines.len() as f32 + 1.0) * 11.0 + 7.0;
            let tx = if mouse[0] + 14.0 + width > gw - 4.0 { mouse[0] - 14.0 - width } else { mouse[0] + 14.0 };
            let ty = (mouse[1] + 12.0).min(gh - TOOL_H - 16.0 - height).max(4.0);
            game.native_fill(batch, [tx - 1.0, ty - 1.0, width + 2.0, height + 2.0], EDGE);
            game.native_fill(batch, [tx, ty, width, height], 0xF010_141C);
            game.native_fill(batch, [tx, ty, 2.0, height], 0xFF00_0000 | w.color);
            for (n, (line, color)) in lines.iter().enumerate() {
                say(game, batch, line, [tx + 8.0, ty + 4.0 + n as f32 * 11.0], *color, true);
            }
            say(game, batch, &keys_line, [tx + 8.0, ty + 6.0 + lines.len() as f32 * 11.0], 0x7FA58A, true);
        }

        // The menu.
        if let Some((kind, at)) = s.menu {
            let rows = kind.labels().len();
            let total = menu_height(rows);
            let (x, y) = (at[0], at[1]);
            let mw = s.menu_w;
            game.native_fill(batch, [x - 1.0, y - 1.0, mw + 2.0, total + 2.0], EDGE);
            game.native_fill(batch, [x, y, mw, total], 0xF010_141C);
            game.native_fill(batch, [x, y, mw, MENU_HEAD], 0xFF18_2230);
            let (head1, head2, tx, tz, tint) = s.menu_head(kind);
            say(game, batch, &head1, [x + 4.0, y + 3.0], tint, true);
            say(game, batch, &head2, [x + 4.0, y + 13.0], MUTED, true);
            let hot = menu_row_at(at, mw, mouse, rows);
            let tags = menu_tags(game, kind);
            let tag_w = tags.iter().flatten().map(|t| width_of(game, t)).fold(0.0_f32, f32::max);
            let tag_w = if tag_w > 0.0 { tag_w + 5.0 } else { 0.0 };
            for (i, label) in kind.labels().iter().enumerate() {
                let top = y + MENU_HEAD + i as f32 * MENU_ROW;
                if hot == Some(i) {
                    game.native_fill(batch, [x, top, mw, MENU_ROW], HOT);
                }
                let danger = matches!(kind, MenuKind::Waypoint(_)) && i == rows - 1;
                let color = if danger { 0xFF8080 } else { TEXT };
                if let Some(Some(tag)) = tags.get(i) {
                    say(game, batch, tag, [x + 6.0, top + 3.0], 0x55FF55, true);
                }
                say(game, batch, label, [x + 6.0 + tag_w, top + 3.0], color, true);
            }
            let far = ((f64::from(tx) + 0.5 - px).powi(2) + (f64::from(tz) + 0.5 - pz).powi(2)).sqrt();
            let foot = if ru() {
                format!("{far:.1} м")
            } else {
                format!("{far:.1} m")
            };
            say(game, batch, &foot,
                [x + 6.0, y + MENU_HEAD + rows as f32 * MENU_ROW + 3.0],
                MUTED,
                true,
            );
        }
        let _ = py;

        // Sharing a waypoint with everyone in the chat: are you sure?
        if s.confirm_share.is_some() {
            let r = s.confirm_rect();
            game.native_fill(batch, [r[0] - 1.0, r[1] - 1.0, r[2] + 2.0, r[3] + 2.0], EDGE);
            game.native_fill(batch, r, 0xF010_141C);
            game.native_fill(batch, [r[0], r[1], r[2], 2.0], 0xFF00_0000 | ACCENT);
            let mid = r[0] + r[2] / 2.0;
            let (a, red, b, c) = if ru() {
                ("Вы уверены, что хотите поделиться этой меткой", "СО ВСЕМИ", " в чате?", "Убедитесь, что тем самым вы не выдадите секрет.")
            } else {
                ("Are you sure you want to share this waypoint", "WITH EVERYONE", " in the chat?", "Make sure you are not giving away a secret.")
            };
            centered_text(game, batch, a, mid, r[1] + 9.0, TEXT);
            let (wr, wb) = (width_of(game, red), width_of(game, b));
            let left = mid - (wr + wb) / 2.0;
            say(game, batch, red, [left.round(), r[1] + 21.0], 0xFF5555, true);
            say(game, batch, b, [(left + wr).round(), r[1] + 21.0], TEXT, true);
            centered_text(game, batch, c, mid, r[1] + 36.0, MUTED);
            let (yes, no) = s.confirm_buttons();
            button(game, batch, yes, inside(yes, mouse), false, true);
            centered_text(game, batch, if ru() { "Да" } else { "Yes" }, yes[0] + yes[2] / 2.0, yes[1] + 5.0, 0xFFFFFF);
            button(game, batch, no, inside(no, mouse), false, false);
            centered_text(game, batch, if ru() { "Нет" } else { "No" }, no[0] + no[2] / 2.0, no[1] + 5.0, TEXT);
        }

        // A name being typed: a field, and Save and Cancel.
        if let Some((name, x, z)) = &s.entering {
            let r = s.dialog_rect();
            game.native_fill(batch, [r[0] - 1.0, r[1] - 1.0, r[2] + 2.0, r[3] + 2.0], EDGE);
            game.native_fill(batch, r, 0xF010_141C);
            game.native_fill(batch, [r[0], r[1], r[2], 2.0], 0xFF00_0000 | ACCENT);
            let head = if s.editing_group {
                tr("Waypoint group").into_owned()
            } else if s.editing.is_some() {
                "Rename waypoint".to_owned()
            } else {
                if ru() {
                    format!("Новая метка: {x}, {z}")
                } else {
                    format!("New waypoint at {x}, {z}")
                }
            };
            centered_text(game, batch, &head, r[0] + r[2] / 2.0, r[1] + 7.0, TEXT);
            let field = [r[0] + 12.0, r[1] + 22.0, r[2] - 24.0, 17.0];
            field_box(game, batch, field, true);
            let chosen = s.dialog_color();
            say(game, batch, &format!("{name}_"),
                [field[0] + 5.0, field[1] + 5.0],
                chosen,
                true,
            );
            // The colours: click one, or Tab for the next.
            for (i, rect) in s.swatches().iter().enumerate().filter(|_| !s.editing_group) {
                let c = COLORS[i];
                let edge = if c == chosen {
                    0xFFFF_FFFF
                } else if inside(*rect, mouse) {
                    0xFF80_90A0
                } else {
                    0xFF00_0000
                };
                game.native_fill(batch, [rect[0] - 1.0, rect[1] - 1.0, rect[2] + 2.0, rect[3] + 2.0], edge);
                game.native_fill(batch, *rect, 0xFF00_0000 | c);
            }
            let (save, cancel) = s.dialog_buttons();
            button(game, batch, save, inside(save, mouse), false, true);
            centered_text(game, batch, "Save", save[0] + save[2] / 2.0, save[1] + 5.0, 0xFFFFFF);
            button(game, batch, cancel, inside(cancel, mouse), false, false);
            centered_text(game, batch, "Cancel", cancel[0] + cancel[2] / 2.0, cancel[1] + 5.0, TEXT);
        }

        // Go to coordinates: a field, and Go and Cancel.
        if let Some(text) = &s.goto {
            let r = s.dialog_rect();
            game.native_fill(batch, [r[0] - 1.0, r[1] - 1.0, r[2] + 2.0, r[3] + 2.0], EDGE);
            game.native_fill(batch, r, 0xF010_141C);
            game.native_fill(batch, [r[0], r[1], r[2], 2.0], 0xFF00_0000 | ACCENT);
            centered_text(
                game,
                batch,
                "Go to coordinates: X Z",
                r[0] + r[2] / 2.0,
                r[1] + 7.0,
                TEXT,
            );
            let field = [r[0] + 12.0, r[1] + 22.0, r[2] - 24.0, 17.0];
            field_box(game, batch, field, true);
            say(game, batch, &format!("{text}_"), [field[0] + 5.0, field[1] + 5.0], WARN, true);
            if text.is_empty() {
                say(game, batch, "for example: 1200 -350",
                    [field[0] + 14.0, field[1] + 5.0],
                    MUTED,
                    true,
                );
            }
            let (go, cancel) = s.dialog_buttons();
            button(game, batch, go, inside(go, mouse), false, true);
            centered_text(game, batch, "Go", go[0] + go[2] / 2.0, go[1] + 5.0, 0xFFFFFF);
            button(game, batch, cancel, inside(cancel, mouse), false, false);
            centered_text(game, batch, "Cancel", cancel[0] + cancel[2] / 2.0, cancel[1] + 5.0, TEXT);
        }

        // A line that says what just happened, for a few seconds.
        if let Some((line, since)) = &s.notice {
            if since.elapsed() < Duration::from_secs(6) {
                let lw = width_of(game, line);
                let x = ((gw - lw) / 2.0).max(6.0);
                panel(game, batch, [x - 8.0, 38.0, lw + 16.0, 14.0]);
                say(game, batch, line, [x, 41.0], TEXT, true);
            }
        }
    });
}

/// The waypoints in the world: a marker with the name and the distance where
/// each one is on the screen.
pub fn hud(game: &Game<'_>, batch: &mut HudBatch) {
    if !game.native_on(ID) || !game.native_window() || !game.native_playing() {
        return;
    }
    if !ours(game) {
        with(|s| s.draw_minimap(game, batch));
    }
    if !with(|s| s.settings.markers && !s.waypoints.is_empty()) {
        return;
    }
    let Some(cam) = camera_of(game, batch.width, batch.height) else {
        return;
    };
    let (gw, gh) = (batch.width, batch.height);
    with(|s| {
        for w in s
            .waypoints
            .iter()
            .filter(|w| s.show_waypoints && w.dim == cam.dimension && s.group_visible(w))
        {
            let rel = [
                f64::from(w.x) + 0.5 - cam.position[0],
                f64::from(w.y) + 1.0 - cam.position[1],
                f64::from(w.z) + 0.5 - cam.position[2],
            ];
            let distance = (rel[0] * rel[0] + rel[1] * rel[1] + rel[2] * rel[2]).sqrt();
            if distance > 20_000.0 {
                continue;
            }
            let v = mul(&cam.view, [rel[0] as f32, rel[1] as f32, rel[2] as f32, 1.0]);
            let clip = mul(&cam.projection, v);
            if clip[3] <= 0.05 {
                continue;
            }
            let (nx, ny) = (clip[0] / clip[3], clip[1] / clip[3]);
            if nx.abs() > 1.1 || ny.abs() > 1.1 {
                continue;
            }
            let (x, y) = ((nx * 0.5 + 0.5) * gw, (0.5 - ny * 0.5) * gh);
            let label = format!("{}  {}m", w.name, distance.round() as i64);
            waypoint_icon(game, batch, x, y, w, &label);
        }
    });
}

/// The camera the world is drawn from, as far as the waypoints need it: where it is, the view
/// and projection matrices (column-major), the dimension.
struct Cam {
    position: [f64; 3],
    view: [f32; 16],
    projection: [f32; 16],
    dimension: String,
}

/// The game hands the camera to mods only when it has any registered, and a native mod may be
/// the only one: then it is worked out here as the game does it.
fn camera_of(game: &Game<'_>, gw: f32, gh: f32) -> Option<Cam> {
    if let Some(cam) = game.native_camera() {
        return Some(Cam {
            position: cam.position,
            view: cam.view,
            projection: cam.projection,
            dimension: cam.dimension,
        });
    }
    let dimension = game.native_player()?.dimension.to_string();
    // The camera of the frame before (the game keeps it for the locator bar): between ticks,
    // with the game's own turn of the view, as it bobs while walking and shakes when hurt.
    if let Some((position, yaw, pitch, fov)) = game.locator_camera {
        let partial = game.clock.camera_fraction() as f32;
        let view = game.bob_matrix(partial)
            * game.spinning_matrix(partial)
            * horizon_client::camera::view_matrix(yaw, pitch);
        return Some(Cam {
            position,
            view: view.to_cols_array(),
            projection: glam::Mat4::perspective_rh(fov.to_radians(), gw / gh.max(1.0), 4096.0, 0.05)
                .to_cols_array(),
            dimension,
        });
    }
    let state = game.native_client_state()?;
    let position = state.eye_position?;
    let fov = game.fov_degrees(state.world_partial_tick as f32, position);
    Some(Cam {
        position,
        view: horizon_client::camera::view_matrix(state.yaw, state.pitch).to_cols_array(),
        projection: glam::Mat4::perspective_rh(fov.to_radians(), gw / gh.max(1.0), 4096.0, 0.05)
            .to_cols_array(),
        dimension,
    })
}

/// The command the green [Add] of a shared place runs (never typed: the game gives it to the mod
/// before it goes anywhere).
const COMMAND_ADD: &str = "worldmap_add";

/// The English text or the Russian, by the game's language.
fn pick(en: impl Into<String>, ru_text: impl Into<String>) -> String {
    if ru() { ru_text.into() } else { en.into() }
}

/// A line of the chat from the mod, with its green tag.
fn chat(game: &mut Game<'_>, line: &str) {
    game.native_chat(&format!("\u{a7}a[Map] \u{a7}f{line}"));
}

/// A place somebody shared, as it is in the chat and in what the [Add] runs.
struct Shared {
    name: String,
    x: i32,
    y: i32,
    z: i32,
    dim: String,
}

/// What is sent to the chat to share a waypoint: plain words, so that a player without the mod
/// reads the place too (`Waypoint "Home" at 100 64 -200 (overworld)`).
fn share_line(w: &Waypoint) -> String {
    let name = w.name.replace(['"', '|'], "'");
    format!(
        "Waypoint \"{name}\" at {} {} {} ({})",
        w.x,
        w.y,
        w.z,
        w.dim.trim_start_matches("minecraft:")
    )
}

/// A shared place in a line of chat: where it starts and ends in the line, and the place.
fn parse_shared(line: &str) -> Option<(usize, usize, Shared)> {
    let start = line.find("Waypoint \"")?;
    let after = &line[start + 10..];
    let quote = after.find("\" at ")?;
    let rest = &after[quote + 5..];
    let close = rest.find(')')?;
    let (numbers, dim) = rest[..close].split_once(" (")?;
    let end = start + 10 + quote + 5 + close + 1;
    let n: Vec<i32> = numbers.split(' ').map(|s| s.parse().ok()).collect::<Option<_>>()?;
    let shared = check_shared(&after[..quote], &n, dim)?;
    Some((start, end, shared))
}

/// What the [Add] runs, `name|x|y|z|dimension`, read back.
fn parse_payload(payload: &str) -> Option<Shared> {
    let mut f = payload.rsplitn(5, '|');
    let dim = f.next()?;
    let z: i32 = f.next()?.parse().ok()?;
    let y: i32 = f.next()?.parse().ok()?;
    let x: i32 = f.next()?.parse().ok()?;
    let name = f.next()?;
    check_shared(name, &[x, y, z], dim)
}

/// A shared place, if it is a sane one: a name, coordinates in the world, a dimension's name.
fn check_shared(name: &str, n: &[i32], dim: &str) -> Option<Shared> {
    let name = name.trim();
    if n.len() != 3
        || name.is_empty()
        || name.chars().count() > 32
        || name.chars().any(char::is_control)
        || name.contains('|')
        || n[0].abs() > 30_000_000
        || n[2].abs() > 30_000_000
        || n[1].abs() > 4096
        || dim.is_empty()
        || dim.len() > 48
        || !dim.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | ':' | '.' | '/' | '-'))
    {
        return None;
    }
    let dim = if dim.contains(':') { dim.to_owned() } else { format!("minecraft:{dim}") };
    Some(Shared { name: name.to_owned(), x: n[0], y: n[1], z: n[2], dim })
}

/// A line of chat from the server, with a shared place in it: the place is told in words and
/// has a green [Add] that adds it to the map. The click is the mod's own command, so nothing
/// goes to the server, and a place is added only when it is clicked.
pub fn chat_received(game: &Game<'_>, message: &mut Text) {
    if !game.native_on(ID) {
        return;
    }
    let plain = message.plain();
    let Some((start, end, shared)) = parse_shared(&plain) else {
        return;
    };
    let upto = plain[..end].chars().count();
    let total = plain.chars().count();
    let sender = sender_of(&plain[..start]);
    let payload = format!("{COMMAND_ADD} {}|{}|{}|{}|{}", shared.name, shared.x, shared.y, shared.z, shared.dim);
    let from = dimension_phrase(&shared.dim);
    let said = if sender.is_empty() {
        pick(
            format!("Shared a waypoint called \"{}\" from {from}! ", shared.name),
            format!("Поделился меткой «{}» из {from}! ", shared.name),
        )
    } else {
        pick(
            format!("{sender} shared a waypoint called \"{}\" from {from}! ", shared.name),
            format!("{sender} поделился меткой «{}» из {from}! ", shared.name),
        )
    };
    let mut out = Text::literal(said);
    out.push(Text::styled(
        pick("[Add]", "[Добавить]"),
        Style {
            bold: Some(true),
            underlined: Some(true),
            click: Some(ClickEvent::RunCommand(payload)),
            hover: Some(Box::new(Text::literal(format!(
                "{} ({} {} {})",
                pick("Click to add this waypoint to your map", "Нажмите, чтобы добавить метку на карту"),
                shared.x,
                shared.y,
                shared.z
            )))),
            ..Style::color(0x55FF55)
        },
    ));
    if upto < total {
        out.push(message.slice(upto, total));
    }
    *message = out;
}

/// Who said a line of chat, from what comes before the place in it: `<Alex> ` or
/// `Alex whispers to you: `.
fn sender_of(prefix: &str) -> String {
    let p = prefix.trim();
    if let Some(rest) = p.strip_prefix('<')
        && let Some((name, _)) = rest.split_once('>')
    {
        return name.trim().to_owned();
    }
    p.split_whitespace().next().unwrap_or("").trim_end_matches(':').to_owned()
}

/// A dimension as it is said in "from the Nether": in the game's language.
fn dimension_phrase(dim: &str) -> String {
    let short = dim.trim_start_matches("minecraft:");
    if ru() {
        match short {
            "overworld" => "Верхнего мира".to_owned(),
            "the_nether" => "Незера".to_owned(),
            "the_end" => "Энда".to_owned(),
            other => other.to_owned(),
        }
    } else {
        short.replace('_', " ")
    }
}

/// The chat commands the mod owns: the chat box colours and completes `/wp` like the game's own.
pub fn commands(game: &Game<'_>, commands: &mut Vec<&'static str>) {
    if game.native_on(ID) && !commands.contains(&"wp") {
        commands.push("wp");
    }
}

/// The chat box's command tree with `/wp`'s words under it (add, del, ...), so that they are
/// completed and listed as the game's own commands' are. What is typed after them goes to
/// the mod as it was.
pub fn extend_tree(mut tree: horizon_client::command::Dispatcher) -> horizon_client::command::Dispatcher {
    use horizon_client::command::{ArgType, MOD_COMMAND};
    let Some(wp) = tree.find("wp") else {
        return tree;
    };
    for word in ["add", "del", "rename", "list", "share", "clear", "help"] {
        let node = tree.literal(wp, word);
        tree.executes(node, MOD_COMMAND);
        let args = tree.argument(node, "args", ArgType::Greedy);
        tree.executes(args, MOD_COMMAND);
    }
    tree
}

/// The mod's own command: the green [Add] of a shared place.
pub fn command(game: &mut Game<'_>, command: &str, ran: &mut bool) {
    if !game.native_on(ID) {
        return;
    }
    let typed = command.trim().trim_start_matches('/');
    // `/wp ...`: the waypoints from the chat.
    let (word, args) = typed.split_once(char::is_whitespace).unwrap_or((typed, ""));
    if word == "wp" {
        *ran = true;
        for line in with(|s| s.wp_command(args)) {
            chat(game, &line);
        }
        if let Some(say) = with(|s| s.pending.take())
            && let Err(e) = game.native_send_chat(&say)
        {
            log::warn!("World Map: could not send {say}: {e}");
        }
        return;
    }
    let Some(payload) = typed.strip_prefix(COMMAND_ADD) else {
        return;
    };
    *ran = true;
    let line = with(|s| s.add_shared(payload.trim()));
    chat(game, &line);
}

/// `m` (column-major, 4 x 4) times `v`.
fn mul(m: &[f32; 16], v: [f32; 4]) -> [f32; 4] {
    let mut out = [0.0; 4];
    for (r, o) in out.iter_mut().enumerate() {
        *o = (0..4).map(|c| m[c * 4 + r] * v[c]).sum();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waypoint_lines_come_from_this_mod_and_from_others() {
        let own = parse_waypoint("Home\t10\t64\t-20\te04040\tminecraft:the_nether\t2\tBase", "x").unwrap();
        assert_eq!((own.name.as_str(), own.x, own.y, own.z), ("Home", 10, 64, -20));
        assert_eq!((own.dim.as_str(), own.shape, own.group.as_str()), ("minecraft:the_nether", 2, "Base"));
        // The files of 0.5.0 had no group.
        let old = parse_waypoint("Mine\t1\t2\t3\t60d060\tminecraft:overworld\t0", "x").unwrap();
        assert_eq!(old.group, "");
        let other = parse_waypoint("waypoint:Farm \u{a7}\u{a7} east:F:100:70:-200:12:false:0:Base:false:0:0:false", "minecraft:overworld").unwrap();
        assert_eq!((other.name.as_str(), other.x, other.y, other.z), ("Farm : east", 100, 70, -200));
        assert_eq!((other.dim.as_str(), other.group.as_str()), ("minecraft:overworld", "Base"));
        assert!(COLORS.contains(&other.color));
        let tilde = parse_waypoint("waypoint:A:A:5:~:7:3:false:0:gui.xaero_default:false:0:0:false", "d").unwrap();
        assert_eq!((tilde.y, tilde.group.as_str()), (64, ""));
        assert!(parse_waypoint("# a comment", "d").is_none());
        assert!(parse_waypoint("", "d").is_none());
    }

    #[test]
    fn a_waypoint_list_is_grouped_when_any_group_is_named() {
        let mut s = State::new();
        s.view_dim = None;
        s.loaded = Some(("w".to_owned(), "minecraft:overworld".to_owned()));
        let w = |name: &str, x: i32, group: &str| Waypoint {
            name: name.to_owned(),
            x,
            y: 64,
            z: 0,
            color: 0xFFFFFF,
            dim: "minecraft:overworld".to_owned(),
            shape: 0,
            group: group.to_owned(),
            hidden: false,
        };
        s.waypoints = vec![w("a", 5, ""), w("b", 1, "Base"), w("c", 9, "Base")];
        let rows = s.list_rows();
        let shape: Vec<String> = rows
            .iter()
            .map(|r| match r {
                ListRow::Group(g) => format!("[{g}]"),
                ListRow::Item(i) => s.waypoints[*i].name.clone(),
            })
            .collect();
        assert_eq!(shape, ["[Base]", "b", "c", "[]", "a"]);
        s.waypoints[1].group.clear();
        s.waypoints[2].group.clear();
        assert_eq!(s.list_rows().len(), 3);
        s.hidden_groups.insert("Nope".to_owned());
        assert!(s.group_visible(&s.waypoints[0]));
    }

    #[test]
    fn a_point_ahead_projects_to_the_middle_of_the_screen() {
        let view = horizon_client::camera::view_matrix(0.0, 0.0).to_cols_array();
        let proj = glam::Mat4::perspective_rh(70f32.to_radians(), 16.0 / 9.0, 4096.0, 0.05)
            .to_cols_array();
        let project = |p: [f32; 3]| mul(&proj, mul(&view, [p[0], p[1], p[2], 1.0]));
        // Yaw 0 faces south (+z): ahead is the middle, behind is not in front, and west (-x)
        // is on the right.
        let ahead = project([0.0, 0.0, 10.0]);
        assert!(ahead[3] > 0.0);
        assert!((ahead[0] / ahead[3]).abs() < 1e-3 && (ahead[1] / ahead[3]).abs() < 1e-3);
        assert!(project([0.0, 0.0, -10.0])[3] < 0.0);
        let west = project([-5.0, 0.0, 10.0]);
        assert!(west[0] / west[3] > 0.1, "{west:?}");
        let up = project([0.0, 3.0, 10.0]);
        assert!(up[1] / up[3] > 0.1, "{up:?}");
    }

    #[test]
    fn a_shared_place_is_told_in_the_chat_and_read_back() {
        let w = Waypoint {
            name: "Home \"base\" | 2".to_owned(),
            x: 100,
            y: 64,
            z: -200,
            color: 0xFFFFFF,
            dim: "minecraft:the_nether".to_owned(),
            shape: 0,
            group: String::new(),
            hidden: false,
        };
        let line = share_line(&w);
        // Quotes and bars of the name are turned into apostrophes: they are what the line is cut by.
        assert_eq!(line, "Waypoint \"Home 'base' ' 2\" at 100 64 -200 (the_nether)");
        // As another player's line shows it: their name before, and the place.
        let said = format!("<Alex> {line} thanks");
        let (start, end, shared) = parse_shared(&said).unwrap();
        assert_eq!(&said[..start], "<Alex> ");
        assert_eq!(&said[end..], " thanks");
        assert_eq!((shared.x, shared.y, shared.z), (100, 64, -200));
        assert_eq!(shared.dim, "minecraft:the_nether");
        // What the [Add] runs.
        let back = parse_payload(&format!("{}|{}|{}|{}|{}", shared.name, shared.x, shared.y, shared.z, shared.dim)).unwrap();
        assert_eq!((back.name, back.x, back.dim), (shared.name, 100, "minecraft:the_nether".to_owned()));
        // What is not a place, or a sane one, is not taken.
        assert!(parse_shared("Waypoint \"\" at 1 2 3 (overworld)").is_none());
        assert!(parse_shared("Waypoint \"A\" at 99999999 2 3 (overworld)").is_none());
        assert!(parse_shared("Waypoint \"A\" at 1 2 (overworld)").is_none());
        assert!(parse_shared("Waypoint \"A\" at 1 2 3 (Over World)").is_none());
        assert!(parse_payload("A|1|2").is_none());
    }

    #[test]
    fn keys_are_short_words_in_the_menu_and_the_sender_is_found() {
        assert_eq!(key_label("key.keyboard.e"), "E");
        assert_eq!(key_label("key.keyboard.delete"), "DEL");
        assert_eq!(key_label("key.keyboard.left.shift"), "SHIFT");
        assert_eq!(key_label("key.keyboard.unknown"), "");
        assert_eq!(key_label("key.mouse.5"), "5");
        assert_eq!(sender_of("<Px226> "), "Px226");
        assert_eq!(sender_of("Px226 whispers to you: "), "Px226");
        assert_eq!(sender_of(""), "");
    }

    #[test]
    fn a_hidden_waypoint_is_not_shown_and_is_kept_in_the_file() {
        let mut s = State::new();
        s.waypoints_of = Some("w".to_owned());
        s.add_shared("Home|1|64|2|overworld");
        s.waypoints[0].hidden = true;
        assert!(!s.group_visible(&s.waypoints[0]));
        let line = s.waypoints_text();
        assert!(line.trim_end().ends_with("\t1"), "{line:?}");
        let back = parse_waypoint(line.trim_end(), "x").unwrap();
        assert!(back.hidden);
        // The files of 0.6 have no such field: shown.
        let old = parse_waypoint("A\t1\t2\t3\tffffff\tminecraft:overworld\t0\tG", "x").unwrap();
        assert!(!old.hidden);
    }

    #[test]
    fn the_chat_tree_gets_wps_words() {
        let base = horizon_client::command::vanilla::game_dispatcher().with_mod_commands(["wp"]);
        let tree = extend_tree(base.clone());
        let wp = tree.find("wp").expect("wp is in the tree");
        // The words are children of it, and the tree without wp is left as it was.
        assert!(tree.find("waypoint").is_some());
        assert_eq!(extend_tree(horizon_client::command::vanilla::game_dispatcher().clone()).find("wp").is_none(), true);
        let _ = wp;
    }

    #[test]
    fn wp_commands_add_rename_list_share_and_remove() {
        let mut s = State::new();
        s.waypoints_of = Some("w".to_owned());
        s.loaded = Some(("w".to_owned(), "minecraft:overworld".to_owned()));
        s.player = (10.5, 64.0, -20.5, 0.0);
        assert!(s.wp_command("add Home")[0].contains("added"));
        assert_eq!((s.waypoints[0].x, s.waypoints[0].y, s.waypoints[0].z), (10, 64, -21));
        // On a waypoint, a new name renames it: they do not stack.
        assert!(s.wp_command("add Base")[0].contains("Renamed"));
        assert_eq!((s.waypoints.len(), s.waypoints[0].name.as_str()), (1, "Base"));
        s.player = (100.5, 64.0, 100.5, 0.0);
        s.wp_command("add");
        assert_eq!(s.waypoints[1].name, "Waypoint 2");
        assert!(s.wp_command("list").len() == 2);
        assert!(s.wp_command("rename 2 Farm")[0].contains("Renamed"));
        assert_eq!(s.waypoints[1].name, "Farm");
        // A share to a player is a private message; to all it is the chat line.
        s.wp_command("share farm Alex_1");
        assert!(s.pending.take().unwrap().starts_with("/msg Alex_1 Waypoint \"Farm\" at 100 64 100 (overworld)"));
        s.wp_command("share 1 all");
        assert!(s.pending.take().unwrap().starts_with("Waypoint \"Base\""));
        assert!(s.wp_command("share 1 a b;c")[0].contains("player"));
        assert!(s.pending.is_none());
        // Nothing is removed by a bare clear; del without a name takes the nearest.
        s.wp_command("clear");
        assert_eq!(s.waypoints.len(), 2);
        assert!(s.wp_command("del")[0].contains("Farm"));
        assert_eq!(s.waypoints.len(), 1);
        assert!(s.wp_command("clear confirm")[0].contains("removed"));
        assert!(s.waypoints.is_empty());
        assert!(s.wp_command("nonsense")[0].contains("/wp"));
    }

    #[test]
    fn a_slow_computer_gets_a_coarser_image_and_a_fast_one_does_not() {
        // 0.00002 ms a pixel, 853 x 533 pixels: 16 ms: sharp.
        assert_eq!(quality_for(0.00002, 853.0, 533.0, 1.0), 1.0);
        // Three times as slow: coarser, by the square root of 3 or so.
        let q = quality_for(0.00006, 853.0, 533.0, 1.0);
        assert!(q > 1.5 && q <= 2.0, "{q}");
        // It does not come back sharp until the need is well below it.
        assert_eq!(quality_for(0.000055, 853.0, 533.0, q), q);
        assert_eq!(quality_for(0.00001, 853.0, 533.0, q), 1.0);
        // Nothing measured yet: as it is.
        assert_eq!(quality_for(0.0, 853.0, 533.0, 1.5), 1.5);
    }

    #[test]
    fn a_shared_place_is_added_once() {
        let mut s = State::new();
        s.waypoints_of = Some("w".to_owned());
        let first = s.add_shared("Home|1|64|2|overworld");
        assert_eq!(s.waypoints.len(), 1);
        assert!(first.contains("added"), "{first}");
        assert_eq!(s.waypoints[0].dim, "minecraft:overworld");
        let again = s.add_shared("Home|1|64|2|overworld");
        assert_eq!(s.waypoints.len(), 1);
        assert!(again.contains("already"), "{again}");
        assert!(s.add_shared("nonsense").contains("not valid"));
        s.waypoints_of = None;
        assert!(s.add_shared("Other|1|64|2|overworld").contains("world"));
    }

    #[test]
    fn a_let_go_map_glides_and_stops() {
        let mut s = State::new();
        s.cx = 0.0;
        s.bpp = 1.0;
        s.glide = (-600.0, 0.0);
        let mut moved = 0.0;
        for _ in 0..200 {
            s.glide_at = Instant::now() - Duration::from_millis(16);
            let before = s.cx;
            s.glide_step();
            assert!(s.cx >= before, "the map went back");
            moved = s.cx;
        }
        // Dragged to the left, it goes on to the right of the world, slowing down to a stop.
        assert!(moved > 50.0 && moved < 600.0, "{moved}");
        assert_eq!(s.glide, (0.0, 0.0));
    }

    #[test]
    fn a_friends_tunnel_is_this_computer() {
        for a in ["127.0.0.1:25580", "localhost:3000", "LOCALHOST", "[::1]:25565", "127.1.2.3"] {
            assert!(is_loopback(a), "{a}");
        }
        for a in ["play.example.com:25565", "192.168.1.20:25580", "10.0.0.2", "[2001:db8::1]:1"] {
            assert!(!is_loopback(a), "{a}");
        }
    }

    #[test]
    fn the_sky_is_light_by_day_and_dark_at_night() {
        assert_eq!(sky_level(6000), DAY);
        assert_eq!(sky_level(18000), 0);
        assert_eq!(sky_level(6000 + 24000 * 3), DAY);
        let dawn = sky_level(0);
        assert!(dawn > 0 && dawn < DAY, "{dawn}");
        // It gets lighter through the morning and darker through the evening.
        assert!(sky_level(1000) >= dawn);
        assert!(sky_level(12500) > sky_level(13500));
        assert!(sky_level(13500) >= sky_level(18000));
    }

    #[test]
    fn matrices_multiply_column_major() {
        // Translation by (1, 2, 3) in column-major order.
        let m = [
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0, 2.0, 3.0, 1.0,
        ];
        assert_eq!(mul(&m, [1.0, 1.0, 1.0, 1.0]), [2.0, 3.0, 4.0, 1.0]);
    }

    #[test]
    fn menu_rows_start_under_the_head() {
        let at = [10.0, 10.0];
        assert_eq!(menu_row_at(at, MENU_W, [12.0, 10.0 + MENU_HEAD + 1.0], 3), Some(0));
        assert_eq!(menu_row_at(at, MENU_W, [12.0, 10.0 + MENU_HEAD + MENU_ROW + 1.0], 3), Some(1));
        assert_eq!(menu_row_at(at, MENU_W, [12.0, 10.0 + 2.0], 3), None);
        assert_eq!(menu_row_at(at, MENU_W, [12.0, 10.0 + MENU_HEAD + 3.0 * MENU_ROW + 1.0], 3), None);
    }
}
