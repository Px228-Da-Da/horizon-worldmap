// Horizon's World Map, a native mod: a full screen map of everything the player
// has explored, kept between visits, with waypoints. The terrain scan and the
// image are `horizon_client::minimap::worldmap_core` (the module this bundle adds
// to the minimap's file); this module is the game side: the hooks' state, the
// screen (drawn and driven through the mod screen hooks), the waypoints, and the
// markers drawn in the world. Its helper files call into it.
use super::Game;
use horizon_client::{
    hud::{HudBatch, MOD_IMAGE_SPRITE},
    minimap::worldmap_core::{FULL, SURFACE, WorldMap, cave_layer, layer_top},
    mod_api::NativeScreenView,
};
use std::{
    cell::RefCell,
    path::PathBuf,
    time::{Duration, Instant},
};

/// The mod's id (its settings and keys), name and version.
pub const ID: &str = "worldmap";
pub const NAME: &str = "World Map";
pub const VERSION: &str = "0.4.1";
pub const DESCRIPTION: &str = "A full screen map of everything you have explored, kept between visits: drag to move, scroll to zoom, right-click for a menu (teleport, waypoints, PNG export). Waypoints show in the world too.";
pub const KEY_NAME: &str = "open";
pub const DEFAULT_KEY: &str = "key.keyboard.m";

/// The mod's keys, as Options lists them: name, label and the default key. The
/// player's choices are kept by the game (`native_mod_key`).
pub const KEYS: [(&str, &str, &str); 9] = [
    ("open", "Open World Map", "key.keyboard.m"),
    ("waypoint", "New Waypoint", "key.keyboard.b"),
    ("caves", "Cave Mode Settings", "key.keyboard.c"),
    ("grid", "Toggle Chunk Grid", "key.keyboard.g"),
    ("center", "Center on Player", "key.keyboard.space"),
    ("zoom_in", "Zoom In (alternative)", "key.keyboard.equal"),
    ("zoom_out", "Zoom Out (alternative)", "key.keyboard.minus"),
    ("waypoints", "Toggle Waypoints", "key.keyboard.h"),
    ("goto", "Go to Coordinates", "key.keyboard.j"),
];

/// CPU time the scan may take per frame.
const SCAN_BUDGET: Duration = Duration::from_micros(1500);
const SAVE_EVERY: Duration = Duration::from_secs(20);
const ZOOM_MIN: f32 = 0.25;
const ZOOM_MAX: f32 = 64.0;
/// Waypoint colours, 0xRRGGBB, in the order new ones take.
const COLORS: [u32; 16] = [
    0xE04040, 0xF08030, 0xE0C030, 0x60D060, 0x20A050, 0x40C0A0, 0x40C0E0, 0x3070F0, 0x8060F0,
    0xC060E0, 0xF070B0, 0xFFFFFF, 0xB0B0B0, 0x606060, 0x804020, 0x80C0FF,
];
const SHAPES: usize = 3;

// The look: graphite panels, a teal accent, soft grey text.
const PANEL: u32 = 0xCC10_141C;
const EDGE: u32 = 0xFF2A_3342;
const HOT: u32 = 0xFF1F_3B44;
const ACCENT: u32 = 0x4FD1C5;
const TEXT: u32 = 0xE6EAF0;
const MUTED: u32 = 0x9AA5B5;
const WARN: u32 = 0xF2B84B;

/// The bottom toolbar: a label and a width each, and the height of every button.
const TOOLS: [(&str, f32); 7] = [
    ("", 24.0),
    ("", 24.0),
    ("Center", 64.0),
    ("Grid", 52.0),
    ("Caves", 60.0),
    ("Waypoint", 74.0),
    ("Export", 60.0),
];
const TOOL_H: f32 = 20.0;
const TOOL_GAP: f32 = 4.0;

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

struct Waypoint {
    name: String,
    x: i32,
    y: i32,
    z: i32,
    color: u32,
    dim: String,
    shape: u8,
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
    /// Waypoints are shown, the coordinates being typed for Go to, where the player was
    /// last alive, and whether they were dead a frame ago.
    show_waypoints: bool,
    goto: Option<String>,
    alive_at: (f64, f64, f64),
    was_dead: bool,
    /// The layer the map shows: `SURFACE` or a cave layer.
    view_layer: i32,
    /// Chunks the game has loaded that the map has not scanned yet.
    waiting: Vec<(i32, i32)>,
    born: Instant,
    last_save: Instant,
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
            show_waypoints: true,
            goto: None,
            alive_at: (0.0, 64.0, 0.0),
            was_dead: false,
            view_layer: SURFACE,
            waiting: Vec::new(),
            born: Instant::now(),
            last_save: Instant::now(),
        }
    }

    /// Write what changed.
    fn flush(&mut self) {
        if let Some(dir) = &self.dir
            && let Err(e) = self.map.save_dirty(dir)
        {
            log::warn!("World Map: could not save the map: {e}");
        }
        if self.wp_dirty {
            self.save_waypoints();
        }
    }

    /// Not in a world: save, and let go of it.
    fn leave(&mut self) {
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
        if self.waypoints_of.as_deref() != Some(key) {
            self.wp_file = base.map(|b| b.join("waypoints.txt"));
            self.waypoints_of = Some(key.to_owned());
            self.load_waypoints();
        }
        let chunks = self.dir.as_ref().map_or(0, |d| self.map.load_dir(d));
        log::info!("World Map: {key} {dim}: {chunks} explored chunks read");
    }

    fn load_waypoints(&mut self) {
        self.waypoints.clear();
        self.wp_dirty = false;
        let Some(path) = &self.wp_file else {
            return;
        };
        let Ok(text) = std::fs::read_to_string(path) else {
            return;
        };
        for line in text.lines() {
            let f: Vec<&str> = line.split('\t').collect();
            if f.len() < 6 {
                continue;
            }
            let (Ok(x), Ok(y), Ok(z), Ok(color)) = (
                f[1].parse::<i32>(),
                f[2].parse::<i32>(),
                f[3].parse::<i32>(),
                u32::from_str_radix(f[4], 16),
            ) else {
                continue;
            };
            let shape = f.get(6).and_then(|s| s.parse::<u8>().ok()).unwrap_or(0);
            self.waypoints.push(Waypoint {
                name: f[0].to_owned(),
                x,
                y,
                z,
                color,
                dim: f[5].to_owned(),
                shape: shape % SHAPES as u8,
            });
        }
    }

    fn save_waypoints(&mut self) {
        self.wp_dirty = false;
        let Some(path) = &self.wp_file else {
            return;
        };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let mut text = String::new();
        for w in &self.waypoints {
            text.push_str(&format!(
                "{}\t{}\t{}\t{}\t{:06x}\t{}\t{}\n",
                w.name, w.x, w.y, w.z, w.color, w.dim, w.shape
            ));
        }
        if let Err(e) = std::fs::write(path, text) {
            log::warn!("World Map: could not save the waypoints: {e}");
        }
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
        self.img_view = None;
        self.zoom_to = self.bpp;
        self.zoom_anchor = None;
        self.map.invalidate();
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

    /// The cave panel: `[x, y, width, height]`, over the toolbar.
    fn cave_rect(&self) -> [f32; 4] {
        let h = CAVE_HEAD + CAVE_ROWS as f32 * CAVE_ROW + CAVE_HELP;
        [
            ((self.gui.0 - CAVE_W) / 2.0).round(),
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
        if self.was_dead || self.loaded.is_none() {
            return;
        }
        self.was_dead = true;
        let (x, y, z) = self.alive_at;
        let count = self
            .waypoints
            .iter()
            .filter(|w| w.name.starts_with("Death"))
            .count();
        let dim = self.dim().to_owned();
        self.waypoints.push(Waypoint {
            name: if count == 0 {
                "Death".to_owned()
            } else {
                format!("Death {}", count + 1)
            },
            x: x.floor() as i32,
            y: y.floor() as i32,
            z: z.floor() as i32,
            color: 0xE04040,
            dim,
            shape: 2,
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
        let scale = (gw.max(gh) / 1024.0).ceil().max(1.0);
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
        if self.map.compose(self.view_layer, ix, iz, b, w, h) {
            Some((w, h, self.map.image.clone()))
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
                format!("chunk {}, {}", x >> 4, z >> 4),
                x,
                z,
                TEXT,
            ),
        }
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
        self.map
            .column(self.view_layer, x, z)
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
            if w.dim != self.dim() {
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
    fn toolbar(&self) -> [[f32; 4]; TOOLS.len()] {
        let total: f32 = TOOLS.iter().map(|t| t.1).sum::<f32>()
            + TOOL_GAP * (TOOLS.len() as f32 - 1.0);
        let mut x = ((self.gui.0 - total) / 2.0).round();
        let y = self.gui.1 - TOOL_H - 6.0;
        let mut out = [[0.0; 4]; TOOLS.len()];
        for (rect, tool) in out.iter_mut().zip(TOOLS) {
            *rect = [x, y, tool.1, TOOL_H];
            x += tool.1 + TOOL_GAP;
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
            _ => self.export(),
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
        let name = format!("{}-{secs}.png", self.dim().trim_start_matches("minecraft:"));
        let path = base.join("exports").join(&name);
        let line = match self.map.export_png(&path) {
            Ok((w, h)) => format!("Saved {name} ({w} x {h}) in the map folder"),
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
                        let (x, y, z) = (w.x, w.y, w.z);
                        self.teleport(x, y, z);
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
                    _ => {
                        self.waypoints.remove(i);
                        self.wp_dirty = true;
                    }
                }
            }
        }
    }

    fn teleport(&mut self, x: i32, y: i32, z: i32) {
        self.pending = Some(format!(
            "/tp @s {} {} {}",
            f64::from(x) + 0.5,
            y,
            f64::from(z) + 0.5
        ));
    }

    /// Rename waypoint `i`: its name in the box.
    fn begin_edit(&mut self, i: usize) {
        let w = &self.waypoints[i];
        self.entering = Some((w.name.clone(), w.x, w.z));
        self.editing = Some(i);
        self.entering_color = None;
        self.skip_char = None;
    }

    fn begin_waypoint(&mut self, at: [f32; 2]) {
        let (x, z) = self.to_world(at);
        self.editing = None;
        self.entering_color = None;
        self.entering = Some((String::new(), x.floor() as i32, z.floor() as i32));
    }

    fn commit_waypoint(&mut self) {
        let Some((name, x, z)) = self.entering.take() else {
            return;
        };
        let name = name.trim().to_owned();
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
            format!("Waypoint {}", self.waypoints.len() + 1)
        } else {
            name
        };
        let y = self.ground_y(x, z).unwrap_or(self.player.1 as i32);
        let color = picked.unwrap_or(COLORS[self.waypoints.len() % COLORS.len()]);
        let dim = self.dim().to_owned();
        self.waypoints.push(Waypoint {
            name,
            x,
            y,
            z,
            color,
            dim,
            shape: 0,
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

/// Whether the map screen is the one open.
fn ours(game: &Game<'_>) -> bool {
    game.native_mod_screen() == Some(ID)
}

/// The world's folder name for the map files: a single player world by its name
/// (the window's handle on its server has it), a server by its address.
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
    let key = world_key(game);
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
    let upload = with(|s| {
        s.player = (player.x, player.y, player.z, player.yaw);
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
        if moved {
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
        s.map.update(
            &game.world,
            game.res.facts(),
            center,
            radius,
            SCAN_BUDGET,
            scan_layer,
        );
        s.waiting = if open && s.view_layer == scan_layer.map_or(SURFACE, |c| c.0) {
            s.map
                .pending(&game.world, center, radius, s.view_layer, 96)
        } else {
            Vec::new()
        };
        if s.last_save.elapsed() >= SAVE_EVERY {
            s.last_save = Instant::now();
            s.flush();
        }
        if open {
            s.ease_zoom();
            s.compose()
        } else {
            None
        }
    });
    if let Some((w, h, rgba)) = upload {
        game.native_mod_image(w as u32, h as u32, &rgba);
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
    if key != game.native_mod_key(ID, KEY_NAME, DEFAULT_KEY) {
        return;
    }
    if ours(game) {
        // Typing a name, the key is a letter.
        let free = with(|s| s.entering.is_none() && s.top_input.is_none() && s.goto.is_none());
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
                // As wide as its widest line, and kept on the screen.
                let (head1, head2, ..) = s.menu_head(kind);
                let mut width = MENU_W;
                for line in [head1.as_str(), head2.as_str()]
                    .into_iter()
                    .chain(kind.labels().iter().copied())
                {
                    width = width.max(game.native_text_width(line) + 14.0);
                }
                s.menu_w = width.min(s.gui.0 - 4.0);
                let height = menu_height(kind.labels().len());
                let at = [
                    mouse[0].min(s.gui.0 - s.menu_w - 2.0).max(2.0),
                    mouse[1].min(s.gui.1 - height - 2.0).max(2.0),
                ];
                s.menu = Some((kind, at));
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
        if s.entering.is_none() && s.menu.is_none() {
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
    if mod_id != ID || button < 1000 {
        return;
    }
    if let Some(key) = KEYS.get(button - 1000) {
        game.native_set_mod_key(ID, key.0, None);
    }
}

/// A key went down on the map screen.
pub fn screen_key(game: &Game<'_>, key: &str, canceled: &mut bool) {
    if !ours(game) {
        return;
    }
    let kb = keys(game);
    with(|s| {
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
        if s.entering.is_some() {
            *canceled = true;
            s.typing_key(key);
            return;
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
                s.goto = Some(String::new());
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
            "key.keyboard.delete" => {
                if let Some(i) = s.waypoint_at(s.mouse) {
                    s.waypoints.remove(i);
                    s.wp_dirty = true;
                }
            }
            // Escape and the rest are the game's.
            _ => *canceled = false,
        }
    });
}

// -- Drawing -------------------------------------------------------------------

/// A waypoint's marker: a diamond, a square or a pennant, with a dark outline.
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

fn centered_text(game: &Game<'_>, batch: &mut HudBatch, text: &str, x: f32, y: f32, rgb: u32) {
    let w = game.native_text_width(text);
    game.native_text(batch, text, [(x - w / 2.0).round(), y.round()], rgb, true);
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
        _ => {
            bar(batch, -1.0, -5.0, 2.0, 5.0);
            game.native_quad(
                batch,
                [[cx - 4.0, cy], [cx + 4.0, cy], [cx, cy + 4.0], [cx, cy + 4.0]],
                argb,
            );
            bar(batch, -5.0, 5.0, 11.0, 1.0);
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
        if free && chunk_px >= 5.0 && mouse[1] < gh - TOOL_H - 8.0 {
            let (x, y) = s.to_screen(f64::from((bx >> 4) * 16), f64::from((bz >> 4) * 16));
            brackets(game, batch, [x, y, chunk_px, chunk_px], 0xE000_0000 | ACCENT);
        }

        // Chunks the game has loaded that are not on the map yet, pulsing.
        if chunk_px >= 3.0 {
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
        let count = s.trail.len();
        for (i, (tx, tz)) in s.trail.iter().enumerate() {
            let (x, y) = s.to_screen(*tx, *tz);
            if x < 0.0 || y < 0.0 || x > gw || y > gh {
                continue;
            }
            let alpha = 0x30 + (i * 0x70 / count.max(1)) as u32;
            game.native_fill(batch, [x - 1.0, y - 1.0, 2.0, 2.0], (alpha << 24) | ACCENT);
        }

        // Waypoints.
        let hovered = if free { s.waypoint_at(mouse) } else { None };
        for w in s.waypoints.iter().filter(|w| s.show_waypoints && w.dim == s.dim()) {
            let (x, y) = s.to_screen(f64::from(w.x) + 0.5, f64::from(w.z) + 0.5);
            if x < -20.0 || y < -20.0 || x > gw + 20.0 || y > gh + 20.0 {
                continue;
            }
            marker(game, batch, x, y, 4.0, w.color, w.shape);
            centered_text(game, batch, &w.name, x, y - 15.0, w.color);
        }

        // The player: a chevron where they stand, pointing where they face.
        let (px, py, pz, yaw) = s.player;
        let (x, y) = s.to_screen(px, pz);
        if x > -10.0 && y > -10.0 && x < gw + 10.0 && y < gh + 10.0 {
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

        // Top left: what is under the pointer.
        let height = s
            .ground_y(bx, bz)
            .map_or_else(|| "?".to_owned(), |y| y.to_string());
        let line1 = format!("X {bx}   Y {height}   Z {bz}");
        let dist = ((f64::from(bx) - px).powi(2) + (f64::from(bz) - pz).powi(2)).sqrt();
        let line2 = format!("chunk {}, {}   {} m from you", bx >> 4, bz >> 4, dist.round() as i64);
        let w1 = game.native_text_width(&line1).max(game.native_text_width(&line2));
        panel(game, batch, [6.0, 6.0, w1 + 14.0, 24.0]);
        game.native_text(batch, &line1, [12.0, 9.0], TEXT, true);
        game.native_text(batch, &line2, [12.0, 19.0], MUTED, true);

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
        let layer_line = if s.view_layer == SURFACE {
            "surface".to_owned()
        } else if s.view_layer == FULL {
            "caves, every height".to_owned()
        } else {
            let hand = if s.top_y.is_some() { " (set)" } else { "" };
            format!("caves, Y up to {}{hand}", layer_top(s.view_layer))
        };
        let line1 = format!(
            "{}   {} chunks",
            s.dim().trim_start_matches("minecraft:"),
            s.map.explored_chunks(s.view_layer)
        );
        let line2 = format!("{:.2}x   {layer_line}", 1.0 / s.bpp);
        let loading =
            (!s.waiting.is_empty()).then(|| format!("Loading {} chunks", s.waiting.len()));
        let mut w2 = game.native_text_width(&line1).max(game.native_text_width(&line2));
        if let Some(text) = &loading {
            w2 = w2.max(game.native_text_width(text));
        }
        let rows = if loading.is_some() { 36.0 } else { 24.0 };
        let bottom = gh - TOOL_H - 12.0;
        panel(game, batch, [6.0, bottom - rows, w2 + 14.0, rows]);
        let top = bottom - rows + 3.0;
        game.native_text(batch, &line1, [12.0, top], TEXT, true);
        game.native_text(batch, &line2, [12.0, top + 10.0], MUTED, true);
        if let Some(text) = &loading {
            game.native_text(batch, text, [12.0, top + 20.0], ACCENT, true);
        }

        // The toolbar.
        let rects = s.toolbar();
        for (i, (rect, tool)) in rects.iter().zip(TOOLS).enumerate() {
            let on = (i == 3 && s.grid) || (i == 4 && (s.cave_panel || s.view_layer != SURFACE));
            let hot = free && inside(*rect, mouse);
            button(game, batch, *rect, hot, on, false);
            let color = if on { ACCENT } else { TEXT };
            let cy = rect[1] + rect[3] / 2.0;
            if tool.0.is_empty() {
                icon(game, batch, i, rect[0] + rect[2] / 2.0, cy, color);
            } else {
                icon(game, batch, i, rect[0] + 10.0, cy, color);
                game.native_text(batch, tool.0, [rect[0] + 19.0, cy - 3.5], color, true);
            }
        }

        // The cave panel: mode, type of layers, and the top Y.
        if s.cave_panel {
            let r = s.cave_rect();
            panel(game, batch, r);
            game.native_text(batch, "Cave map", [r[0] + 8.0, r[1] + 4.0], TEXT, true);
            let hot = if free { s.cave_row_at(mouse) } else { None };
            let top_text = match (&s.top_input, s.top_y) {
                (Some(text), _) => format!("{text}_"),
                (None, Some(y)) => y.to_string(),
                (None, None) => format!("auto ({})", s.effective_top()),
            };
            let rows = [
                format!("Cave mode: {}", ["auto", "on", "off"][usize::from(s.cave_mode % 3)]),
                format!(
                    "Type: {}",
                    if s.cave_type == 1 { "full" } else { "layered" }
                ),
                format!("Top Y: {top_text}"),
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
                        game.native_text(batch, &top_text, [field[0] + 5.0, y + 4.0], color, true);
                        let auto_hot = free && inside(auto, mouse);
                        button(game, batch, auto, auto_hot, s.top_y.is_none(), false);
                        let auto_color = if s.top_y.is_none() { ACCENT } else { ink };
                        centered_text(game, batch, "Auto", auto[0] + auto[2] / 2.0, y + 4.0, auto_color);
                    }
                    _ => {
                        // The top, from the world's bottom to its top.
                        game.native_text(batch, "-64", [r[0] + 8.0, y + 4.0], MUTED, true);
                        let high = "319";
                        let hw = game.native_text_width(high);
                        game.native_text(batch, high, [r[0] + r[2] - 8.0 - hw, y + 4.0], MUTED, true);
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
                game.native_text(batch, line, [r[0] + 8.0, base + i as f32 * 10.0], MUTED, true);
            }
        }

        // The waypoint under the pointer.
        if let Some(i) = hovered {
            let w = &s.waypoints[i];
            let line = format!("{}  ({}, {}, {})", w.name, w.x, w.y, w.z);
            let lw = game.native_text_width(&line);
            let tx = (mouse[0] + 10.0).min(gw - lw - 10.0).max(4.0);
            let ty = (mouse[1] + 10.0).min(gh - 40.0);
            panel(game, batch, [tx - 4.0, ty - 3.0, lw + 10.0, 13.0]);
            game.native_text(batch, &line, [tx + 2.0, ty], w.color, true);
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
            game.native_text(batch, &head1, [x + 4.0, y + 3.0], tint, true);
            game.native_text(batch, &head2, [x + 4.0, y + 13.0], MUTED, true);
            let hot = menu_row_at(at, mw, mouse, rows);
            for (i, label) in kind.labels().iter().enumerate() {
                let top = y + MENU_HEAD + i as f32 * MENU_ROW;
                if hot == Some(i) {
                    game.native_fill(batch, [x, top, mw, MENU_ROW], HOT);
                }
                let danger = matches!(kind, MenuKind::Waypoint(_)) && i == rows - 1;
                let color = if danger { 0xFF8080 } else { TEXT };
                game.native_text(batch, label, [x + 6.0, top + 3.0], color, true);
            }
            let far = ((f64::from(tx) + 0.5 - px).powi(2) + (f64::from(tz) + 0.5 - pz).powi(2)).sqrt();
            let foot = format!("{far:.1} m");
            game.native_text(
                batch,
                &foot,
                [x + 6.0, y + MENU_HEAD + rows as f32 * MENU_ROW + 3.0],
                MUTED,
                true,
            );
        }
        let _ = py;

        // A name being typed: a field, and Save and Cancel.
        if let Some((name, x, z)) = &s.entering {
            let r = s.dialog_rect();
            game.native_fill(batch, [r[0] - 1.0, r[1] - 1.0, r[2] + 2.0, r[3] + 2.0], EDGE);
            game.native_fill(batch, r, 0xF010_141C);
            game.native_fill(batch, [r[0], r[1], r[2], 2.0], 0xFF00_0000 | ACCENT);
            let head = if s.editing.is_some() {
                "Rename waypoint".to_owned()
            } else {
                format!("New waypoint at {x}, {z}")
            };
            centered_text(game, batch, &head, r[0] + r[2] / 2.0, r[1] + 7.0, TEXT);
            let field = [r[0] + 12.0, r[1] + 22.0, r[2] - 24.0, 17.0];
            field_box(game, batch, field, true);
            let chosen = s.dialog_color();
            game.native_text(
                batch,
                &format!("{name}_"),
                [field[0] + 5.0, field[1] + 5.0],
                chosen,
                true,
            );
            // The colours: click one, or Tab for the next.
            for (i, rect) in s.swatches().iter().enumerate() {
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
            game.native_text(batch, &format!("{text}_"), [field[0] + 5.0, field[1] + 5.0], WARN, true);
            let (go, cancel) = s.dialog_buttons();
            button(game, batch, go, inside(go, mouse), false, true);
            centered_text(game, batch, "Go", go[0] + go[2] / 2.0, go[1] + 5.0, 0xFFFFFF);
            button(game, batch, cancel, inside(cancel, mouse), false, false);
            centered_text(game, batch, "Cancel", cancel[0] + cancel[2] / 2.0, cancel[1] + 5.0, TEXT);
        }

        // A line that says what just happened, for a few seconds.
        if let Some((line, since)) = &s.notice {
            if since.elapsed() < Duration::from_secs(6) {
                let lw = game.native_text_width(line);
                let x = ((gw - lw) / 2.0).max(6.0);
                panel(game, batch, [x - 8.0, 38.0, lw + 16.0, 14.0]);
                game.native_text(batch, line, [x, 41.0], TEXT, true);
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
    let Some(cam) = game.native_camera() else {
        return;
    };
    let (gw, gh) = (batch.width, batch.height);
    with(|s| {
        for w in s.waypoints.iter().filter(|w| s.show_waypoints && w.dim == cam.dimension) {
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
            marker(game, batch, x, y, 3.0, w.color, w.shape);
            let label = format!("{}  {}m", w.name, distance.round() as i64);
            centered_text(game, batch, &label, x, y - 14.0, w.color);
        }
    });
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
