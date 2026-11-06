// World Map core: the terrain the player has seen, kept between visits, and the
// image drawn from it. It lives in the minimap's file to reuse its column scan
// (`scan_tile`) and its map-item colours and shading. One cell per chunk the
// player has had loaded, in layers: the surface, and cave layers (the floor
// below a height, in slices of 16 blocks), on disk in 32 x 32 chunk regions.
use super::{
    BlockFacts, Layer, Palette, WALL, World, brightness, packed_rgba, ring_offsets, scan_tile,
};
use std::{
    collections::{HashMap, HashSet},
    fs, io,
    path::Path,
    time::{Duration, Instant},
};

/// Chunks per side of a region file.
pub const REGION: i32 = 32;
/// The surface's layer id; a cave layer's is its slice (the height divided by 16).
pub const SURFACE: i32 = i32::MAX;
/// The layer of "full" caves: one map for every height.
pub const FULL: i32 = i32::MIN;
/// The minimap's own layer underground: the floor below the player's head, cut again when
/// their height changes. It is not kept on disk.
pub const LIVE: i32 = i32::MIN + 1;
const MAGIC: &[u8; 6] = b"HZWM2\n";
/// The first format, without the light of each column; still read.
const MAGIC_V1: &[u8; 6] = b"HZWM1\n";
/// Full daylight: the map is drawn as it is. Less is a darker sky, in steps.
pub const DAY: u8 = 32;
/// What is left of a colour in the darkest night, of 256.
const NIGHT_FLOOR: u32 = 72;
/// A loaded chunk is scanned again after a block change at most this often.
const RESCAN_AFTER: Duration = Duration::from_secs(3);
/// Never seen (or void).
const UNEXPLORED: [u8; 4] = [14, 14, 18, 255];
/// Bytes of a cell on disk: chunk x and z, colours, heights, water depths, block light.
const CELL_BYTES: usize = 8 + 256 + 512 + 256 + 256;
/// The same in the first format, which had no block light.
const CELL_BYTES_V1: usize = CELL_BYTES - 256;
/// Bytes of a cell's block light, 8 x 8, 4 x 4, 2 x 2 and 1 x 1, and where each starts.
const GLOW_MIP_BYTES: usize = 64 + 16 + 4 + 1;
const GLOW_MIP_OFFSET: [usize; 5] = [0, 0, 64, 80, 84];
/// Bytes of a cell's smaller pictures of itself, 8 x 8, 4 x 4, 2 x 2 and 1 x 1
/// pixels of RGBA, and where each starts, by level (1 to 4).
const MIP_BYTES: usize = 256 + 64 + 16 + 4;
const MIP_OFFSET: [usize; 5] = [0, 0, 256, 320, 336];

/// Bytes of a cell's full picture: 16 x 16 pixels of RGBA.
const PICTURE_BYTES: usize = 1024;

/// The cell shaded as the map is: a column's north is the column above it, in the cell
/// or in the cell north of it. The map draws from this picture as it is when it shows
/// less than 2 blocks to a pixel, so drawing it costs a copy a pixel.
fn shade(
    cells: &HashMap<(i32, i32, i32), Cell>,
    id: (i32, i32, i32),
    cell: &Cell,
) -> [u8; PICTURE_BYTES] {
    let north_cell = cells.get(&(id.0, id.1, id.2 - 1));
    let mut out = [0u8; PICTURE_BYTES];
    for z in 0..16usize {
        for x in 0..16usize {
            let i = z * 16 + x;
            let c = cell.color[i];
            let rgba = if c & !WALL != 0 {
                let north = if z > 0 {
                    (cell.color[i - 16] & !WALL != 0).then(|| cell.height[i - 16])
                } else {
                    north_cell
                        .and_then(|n| (n.color[240 + x] & !WALL != 0).then(|| n.height[240 + x]))
                };
                packed_rgba(
                    c & !WALL,
                    brightness(c, cell.height[i], north, cell.depth[i], (x + z) as i32),
                )
            } else {
                UNEXPLORED
            };
            out[i * 4..i * 4 + 4].copy_from_slice(&rgba);
        }
    }
    out
}

/// The picture half the size four times over: the pictures the map draws from when it
/// shows 2 blocks or more to a pixel. Each pixel is the mean of four, so a far map
/// blurs instead of flickering.
fn build_mips(picture: &[u8; PICTURE_BYTES]) -> [u8; MIP_BYTES] {
    let mut prev: Vec<[u8; 4]> = picture
        .chunks_exact(4)
        .map(|p| [p[0], p[1], p[2], p[3]])
        .collect();
    let mut out = [0u8; MIP_BYTES];
    let mut size = 16usize;
    for level in 1..=4usize {
        let n = size / 2;
        let mut next: Vec<[u8; 4]> = Vec::with_capacity(n * n);
        for z in 0..n {
            for x in 0..n {
                let mut sum = [0u32; 4];
                for (dx, dz) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let p = prev[(z * 2 + dz) * size + x * 2 + dx];
                    for k in 0..4 {
                        sum[k] += u32::from(p[k]);
                    }
                }
                next.push([
                    (sum[0] / 4) as u8,
                    (sum[1] / 4) as u8,
                    (sum[2] / 4) as u8,
                    (sum[3] / 4) as u8,
                ]);
            }
        }
        let at = MIP_OFFSET[level];
        for (j, p) in next.iter().enumerate() {
            out[at + j * 4..at + j * 4 + 4].copy_from_slice(p);
        }
        prev = next;
        size = n;
    }
    out
}

/// The block light halved four times over, a mean of four each: what a far map lights from.
fn build_glow_mips(glow: &[u8; 256]) -> [u8; GLOW_MIP_BYTES] {
    let mut prev: Vec<u8> = glow.to_vec();
    let mut out = [0u8; GLOW_MIP_BYTES];
    let mut size = 16usize;
    for level in 1..=4usize {
        let n = size / 2;
        let mut next = Vec::with_capacity(n * n);
        for z in 0..n {
            for x in 0..n {
                let mut sum = 0u32;
                for (dx, dz) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    sum += u32::from(prev[(z * 2 + dz) * size + x * 2 + dx]);
                }
                next.push(sum.div_ceil(4) as u8);
            }
        }
        out[GLOW_MIP_OFFSET[level]..GLOW_MIP_OFFSET[level] + next.len()].copy_from_slice(&next);
        prev = next;
        size = n;
    }
    out
}

/// A colour in the dark: the sky's `light` of `DAY`, or the block's own `glow` (0 to 15),
/// whichever is more, keeps it from going down to `NIGHT_FLOOR`.
fn lit(rgba: [u8; 4], glow: u8, light: u8) -> [u8; 4] {
    if light >= DAY || rgba == UNEXPLORED {
        return rgba;
    }
    let own = (u32::from(glow) * u32::from(DAY)).div_ceil(15);
    let f = u32::from(light).max(own).min(u32::from(DAY));
    let m = NIGHT_FLOOR + (256 - NIGHT_FLOOR) * f / u32::from(DAY);
    let ch = |c: u8| (u32::from(c) * m / 256) as u8;
    [ch(rgba[0]), ch(rgba[1]), ch(rgba[2]), rgba[3]]
}

/// Shade a cell again, and the cell south of it, whose first row looks at its last.
fn refresh(cells: &mut HashMap<(i32, i32, i32), Cell>, id: (i32, i32, i32)) {
    for id in [id, (id.0, id.1, id.2 + 1)] {
        let Some(cell) = cells.get(&id) else {
            continue;
        };
        let picture = shade(cells, id, cell);
        let mips = build_mips(&picture);
        if let Some(c) = cells.get_mut(&id) {
            c.rgba = picture;
            c.mips = mips;
        }
    }
}

/// The cave layer a height is in.
pub fn cave_layer(y: i32) -> i32 {
    y.div_euclid(16)
}

/// The highest block a cave layer shows.
pub fn layer_top(layer: i32) -> i32 {
    layer * 16 + 15
}

#[derive(Clone)]
struct Cell {
    color: [u8; 256],
    height: [i16; 256],
    depth: [u8; 256],
    /// The block light (0 to 15) at the top of each column, so a lit block shines at night.
    glow: [u8; 256],
    glow_mips: [u8; GLOW_MIP_BYTES],
    /// The height it was cut at (the live layer's: it is read again when that changes).
    head: i32,
    /// The cell as the map draws it, and smaller.
    rgba: [u8; PICTURE_BYTES],
    mips: [u8; MIP_BYTES],
    /// The world revision it was scanned at (`u64::MAX` when read from disk, so
    /// the chunk's first load scans it again).
    revision: u64,
    scanned: Instant,
}

/// The region a chunk is stored in.
pub fn region_of(chunk: (i32, i32)) -> (i32, i32) {
    (chunk.0.div_euclid(REGION), chunk.1.div_euclid(REGION))
}

#[derive(Default)]
pub struct WorldMap {
    /// By layer, chunk x and chunk z.
    cells: HashMap<(i32, i32, i32), Cell>,
    palette: Option<Palette>,
    /// Regions with cells not yet saved: layer, region x and z.
    dirty: HashSet<(i32, i32, i32)>,
    /// A cell changed since the image was composed.
    changed: bool,
    composed: Option<(i32, f64, f64, f32, usize, usize, u8, i32)>,
    /// The composed image, RGBA.
    pub image: Vec<u8>,
}

impl WorldMap {
    /// Forget everything in memory (another world or dimension).
    pub fn clear(&mut self) {
        let palette = self.palette.take();
        *self = Self {
            palette,
            ..Self::default()
        };
    }

    /// Make the next `compose` draw again, whatever the view.
    pub fn invalidate(&mut self) {
        self.composed = None;
    }

    /// The chunks explored in `layer`.
    pub fn explored_chunks(&self, layer: i32) -> usize {
        self.cells.keys().filter(|k| k.0 == layer).count()
    }

    pub fn has_unsaved(&self) -> bool {
        !self.dirty.is_empty()
    }

    /// The map colour, height and water depth of a block column, if explored.
    pub fn column(&self, layer: i32, x: i32, z: i32) -> Option<(u8, i16, u8)> {
        let cell = self.cells.get(&(layer, x >> 4, z >> 4))?;
        let i = ((z & 15) * 16 + (x & 15)) as usize;
        Some((cell.color[i], cell.height[i], cell.depth[i]))
    }

    /// The loaded chunks around `center` that `layer` has no cell for yet,
    /// nearest first, at most `limit`.
    pub fn pending(
        &self,
        world: &World,
        center: (i32, i32),
        radius: i32,
        layer: i32,
        limit: usize,
    ) -> Vec<(i32, i32)> {
        let mut out = Vec::new();
        for ring in 0..=radius {
            for (dx, dz) in ring_offsets(ring) {
                let key = (center.0 + dx, center.1 + dz);
                if world.chunks.contains_key(&key) && !self.cells.contains_key(&(layer, key.0, key.1))
                {
                    out.push(key);
                    if out.len() >= limit {
                        return out;
                    }
                }
            }
        }
        out
    }

    /// Scan the loaded chunks around `center` (chunk coordinates), nearest first,
    /// the new and the changed ones, until `budget` is spent: the surface, and
    /// the cave layer `cave` if there is one (its id, and the highest block it
    /// shows). Returns how many.
    pub fn update(
        &mut self,
        world: &World,
        facts: &BlockFacts,
        center: (i32, i32),
        radius: i32,
        budget: Duration,
        cave: Option<(i32, i32)>,
        live: Option<(i32, i32)>,
    ) -> usize {
        let palette = self.palette.get_or_insert_with(|| Palette::new(facts));
        let started = Instant::now();
        let air = world.air.0;
        let mut layers = vec![(SURFACE, 0)];
        layers.extend(cave);
        // The live layer: the height of the player's head, and how far round them it reaches.
        let live_reach = live.map_or(-1, |l| l.1);
        layers.extend(live.map(|l| (LIVE, l.0)));
        let mut scanned = 0;
        for ring in 0..=radius {
            for (dx, dz) in ring_offsets(ring) {
                let key = (center.0 + dx, center.1 + dz);
                let Some(chunk) = world.chunks.get(&key) else {
                    continue;
                };
                for &(layer, top) in &layers {
                    if layer == LIVE && ring > live_reach {
                        continue;
                    }
                    let id = (layer, key.0, key.1);
                    let stale = match self.cells.get(&id) {
                        None => true,
                        Some(c) => {
                            (c.revision != world.revision && c.scanned.elapsed() >= RESCAN_AFTER)
                                || (layer == LIVE && c.head != top)
                        }
                    };
                    if !stale {
                        continue;
                    }
                    let what = if layer == SURFACE {
                        Layer::Surface
                    } else {
                        Layer::Below(top)
                    };
                    let tile = scan_tile(chunk, palette, air, what, world.revision);
                    // Block light at the top of each column: its own and the air above.
                    let mut glow = [0u8; 256];
                    for (i, g) in glow.iter_mut().enumerate() {
                        if tile.color[i] & !WALL == 0 {
                            continue;
                        }
                        let (x, z) = ((i & 15) as i32, (i >> 4) as i32);
                        let y = i32::from(tile.height[i]);
                        *g = (chunk.light(x, y, z) & 15).max(chunk.light(x, y + 1, z) & 15);
                    }
                    let different = self.cells.get(&id).is_none_or(|c| {
                        c.color != tile.color
                            || c.height != tile.height
                            || c.depth != tile.depth
                            || c.glow != glow
                    });
                    if different || (layer == LIVE && self.cells.get(&id).is_none_or(|c| c.head != top)) {
                        self.changed = true;
                        if layer != LIVE {
                            let region = region_of(key);
                            self.dirty.insert((layer, region.0, region.1));
                        }
                    }
                    self.cells.insert(
                        id,
                        Cell {
                            color: tile.color,
                            height: tile.height,
                            depth: tile.depth,
                            glow,
                            glow_mips: build_glow_mips(&glow),
                            head: top,
                            rgba: [0; PICTURE_BYTES],
                            mips: [0; MIP_BYTES],
                            revision: tile.revision,
                            scanned: tile.scanned,
                        },
                    );
                    refresh(&mut self.cells, id);
                    scanned += 1;
                    if started.elapsed() >= budget {
                        return scanned;
                    }
                }
            }
        }
        scanned
    }

    /// Draw the `w` x `h` image of `layer` around block (`cx`, `cz`), `bpp`
    /// blocks per pixel, north up, under a sky of `light` of `DAY`. False when nothing
    /// differs from the last one.
    /// With `yaw` (degrees, 0 facing south) the image is turned so that where that points is up.
    #[allow(clippy::too_many_arguments)]
    pub fn compose(
        &mut self,
        layer: i32,
        cx: f64,
        cz: f64,
        bpp: f32,
        w: usize,
        h: usize,
        light: u8,
        yaw: Option<f32>,
    ) -> bool {
        let key = (layer, cx, cz, bpp, w, h, light, yaw.map_or(i32::MIN, |y| (y * 2.0).round() as i32));
        if !self.changed && self.composed == Some(key) && self.image.len() == w * h * 4 {
            return false;
        }
        self.changed = false;
        self.composed = Some(key);
        let mut img = std::mem::take(&mut self.image);
        match yaw {
            Some(yaw) => self.render_turned(layer, cx, cz, bpp, w, h, light, yaw, &mut img),
            None => self.render(layer, cx, cz, bpp, w, h, light, &mut img),
        }
        self.image = img;
        true
    }

    /// The chunks explored on the surface: the lowest and highest chunk x and z.
    pub fn bounds(&self) -> Option<(i32, i32, i32, i32)> {
        let mut keys = self.cells.keys().filter(|k| k.0 == SURFACE);
        let first = keys.next()?;
        Some(keys.fold((first.1, first.2, first.1, first.2), |b, k| {
            (b.0.min(k.1), b.1.min(k.2), b.2.max(k.1), b.3.max(k.2))
        }))
    }

    /// Write the whole explored surface to `path` as a PNG, a block a pixel (or
    /// coarser when it would pass 4096 pixels a side). Returns its size.
    pub fn export_png(&self, path: &Path) -> io::Result<(usize, usize)> {
        let Some((x0, z0, x1, z1)) = self.bounds() else {
            return Err(io::Error::other("nothing explored yet"));
        };
        let blocks_w = ((x1 - x0 + 1) * 16) as usize;
        let blocks_h = ((z1 - z0 + 1) * 16) as usize;
        let scale = blocks_w.max(blocks_h).div_ceil(4096).max(1);
        let (w, h) = (blocks_w.div_ceil(scale), blocks_h.div_ceil(scale));
        let cx = f64::from(x0 * 16) + blocks_w as f64 / 2.0;
        let cz = f64::from(z0 * 16) + blocks_h as f64 / 2.0;
        let mut img = Vec::new();
        self.render(SURFACE, cx, cz, scale as f32, w, h, DAY, &mut img);
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let file = io::BufWriter::new(fs::File::create(path)?);
        let mut encoder = png::Encoder::new(file, w as u32, h as u32);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|e| io::Error::other(e.to_string()))?;
        writer
            .write_image_data(&img)
            .map_err(|e| io::Error::other(e.to_string()))?;
        Ok((w, h))
    }

    /// The same turned by `yaw` degrees: the way a player faces is up (the minimap's).
    #[allow(clippy::too_many_arguments)]
    fn render_turned(
        &self,
        layer: i32,
        cx: f64,
        cz: f64,
        bpp: f32,
        w: usize,
        h: usize,
        light: u8,
        yaw: f32,
        img: &mut Vec<u8>,
    ) {
        img.resize(w * h * 4, 0);
        let (sin, cos) = f64::from(yaw).to_radians().sin_cos();
        // Forward, in the world's x and z; right of it is (-fz, fx).
        let (fx, fz) = (-sin, cos);
        let bpp = f64::from(bpp);
        let (hw, hh) = (w as f64 / 2.0, h as f64 / 2.0);
        // Far, from the cells' smaller pictures; near, from their own.
        let level = if bpp >= 2.0 { (bpp.log2().floor() as usize).clamp(1, 4) } else { 0 };
        let size = 16usize >> level;
        let mut last: Option<((i32, i32), Option<&Cell>)> = None;
        for j in 0..h {
            let v = (j as f64 + 0.5 - hh) * bpp;
            for i in 0..w {
                let u = (i as f64 + 0.5 - hw) * bpp;
                let x = (cx - fz * u - fx * v).floor() as i32;
                let z = (cz + fx * u - fz * v).floor() as i32;
                let key = (x >> 4, z >> 4);
                let cell = match last {
                    Some((k, c)) if k == key => c,
                    _ => {
                        let c = self.cells.get(&(layer, key.0, key.1));
                        last = Some((key, c));
                        c
                    }
                };
                let rgba = match cell {
                    None => UNEXPLORED,
                    Some(c) if level == 0 => {
                        let at = ((z & 15) * 16 + (x & 15)) as usize;
                        lit(
                            [c.rgba[at * 4], c.rgba[at * 4 + 1], c.rgba[at * 4 + 2], c.rgba[at * 4 + 3]],
                            c.glow[at],
                            light,
                        )
                    }
                    Some(c) => {
                        let (lx, lz) = (((x & 15) as usize) >> level, ((z & 15) as usize) >> level);
                        let at = MIP_OFFSET[level] + (lz * size + lx) * 4;
                        let glow = c.glow_mips[GLOW_MIP_OFFSET[level] + lz * size + lx];
                        lit([c.mips[at], c.mips[at + 1], c.mips[at + 2], c.mips[at + 3]], glow, light)
                    }
                };
                let o = (j * w + i) * 4;
                img[o..o + 4].copy_from_slice(&rgba);
            }
        }
    }

    /// The `w` x `h` RGBA image of `layer` around block (`cx`, `cz`) into `img`.
    #[allow(clippy::too_many_arguments)]
    fn render(
        &self,
        layer: i32,
        cx: f64,
        cz: f64,
        bpp: f32,
        w: usize,
        h: usize,
        light: u8,
        img: &mut Vec<u8>,
    ) {
        img.resize(w * h * 4, 0);
        let cells = &self.cells;
        let bpp = f64::from(bpp);
        let (half_w, half_h) = (w as f64 / 2.0, h as f64 / 2.0);
        if bpp >= 2.0 {
            // Far: each pixel is a mean of the cell's blocks, from its smaller pictures.
            let level = (bpp.log2().floor() as usize).clamp(1, 4);
            let size = 16usize >> level;
            let mut last: Option<((i32, i32), Option<&Cell>)> = None;
            for j in 0..h {
                let z = (cz + (j as f64 + 0.5 - half_h) * bpp).floor() as i32;
                for i in 0..w {
                    let x = (cx + (i as f64 + 0.5 - half_w) * bpp).floor() as i32;
                    let key = (x >> 4, z >> 4);
                    let cell = match last {
                        Some((k, c)) if k == key => c,
                        _ => {
                            let c = cells.get(&(layer, key.0, key.1));
                            last = Some((key, c));
                            c
                        }
                    };
                    let rgba = match cell {
                        Some(c) => {
                            let (lx, lz) = (((x & 15) as usize) >> level, ((z & 15) as usize) >> level);
                            let at = MIP_OFFSET[level] + (lz * size + lx) * 4;
                            let glow = c.glow_mips[GLOW_MIP_OFFSET[level] + lz * size + lx];
                            lit(
                                [c.mips[at], c.mips[at + 1], c.mips[at + 2], c.mips[at + 3]],
                                glow,
                                light,
                            )
                        }
                        None => UNEXPLORED,
                    };
                    let o = (j * w + i) * 4;
                    img[o..o + 4].copy_from_slice(&rgba);
                }
            }
            return;
        }
        // Near: each pixel is a block, or part of one, from the cell's own picture.
        let mut last: Option<((i32, i32), Option<&Cell>)> = None;
        for j in 0..h {
            let z = (cz + (j as f64 + 0.5 - half_h) * bpp).floor() as i32;
            for i in 0..w {
                let x = (cx + (i as f64 + 0.5 - half_w) * bpp).floor() as i32;
                let key = (x >> 4, z >> 4);
                let cell = match last {
                    Some((k, c)) if k == key => c,
                    _ => {
                        let c = cells.get(&(layer, key.0, key.1));
                        last = Some((key, c));
                        c
                    }
                };
                let o = (j * w + i) * 4;
                match cell {
                    Some(c) => {
                        let cell_at = ((z & 15) * 16 + (x & 15)) as usize;
                        let at = cell_at * 4;
                        let rgba = [c.rgba[at], c.rgba[at + 1], c.rgba[at + 2], c.rgba[at + 3]];
                        img[o..o + 4].copy_from_slice(&lit(rgba, c.glow[cell_at], light));
                    }
                    None => img[o..o + 4].copy_from_slice(&UNEXPLORED),
                }
            }
        }
    }

    /// Read every region file of `dir` (cells already in memory win). Returns
    /// the chunks read.
    pub fn load_dir(&mut self, dir: &Path) -> usize {
        let Ok(entries) = fs::read_dir(dir) else {
            return 0;
        };
        let mut loaded = 0;
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let Some((layer, _)) = parse_region_name(&name) else {
                continue;
            };
            let Ok(bytes) = fs::read(entry.path()) else {
                continue;
            };
            for (key, cell) in decode_region(&bytes) {
                if let std::collections::hash_map::Entry::Vacant(v) =
                    self.cells.entry((layer, key.0, key.1))
                {
                    v.insert(cell);
                    loaded += 1;
                }
            }
        }
        // Every cell is shaded, now that its neighbours are there.
        let ids: Vec<(i32, i32, i32)> = self.cells.keys().copied().collect();
        for id in ids {
            let Some(cell) = self.cells.get(&id) else {
                continue;
            };
            let picture = shade(&self.cells, id, cell);
            let mips = build_mips(&picture);
            if let Some(c) = self.cells.get_mut(&id) {
                c.rgba = picture;
                c.mips = mips;
            }
        }
        self.changed = true;
        loaded
    }

    /// Write the regions that changed. Returns how many.
    pub fn save_dirty(&mut self, dir: &Path) -> io::Result<usize> {
        if self.dirty.is_empty() {
            return Ok(0);
        }
        fs::create_dir_all(dir)?;
        let regions: Vec<(i32, i32, i32)> = self.dirty.drain().collect();
        let mut written = 0;
        let mut failed = None;
        for region in regions {
            let stem = region_name(region.0, (region.1, region.2));
            let path = dir.join(format!("{stem}.bin"));
            let temp = dir.join(format!("{stem}.tmp"));
            let bytes = encode_region(&self.cells, region.0, (region.1, region.2));
            let result = fs::write(&temp, &bytes).and_then(|()| fs::rename(&temp, &path));
            match result {
                Ok(()) => written += 1,
                Err(e) => {
                    self.dirty.insert(region);
                    failed = Some(e);
                }
            }
        }
        match failed {
            Some(e) => Err(e),
            None => Ok(written),
        }
    }
}

/// A region file's name without its extension: `r.X.Z` for the surface,
/// `c.LAYER.X.Z` for a cave layer.
fn region_name(layer: i32, region: (i32, i32)) -> String {
    if layer == SURFACE {
        format!("r.{}.{}", region.0, region.1)
    } else {
        format!("c.{layer}.{}.{}", region.0, region.1)
    }
}

/// The layer and region a region file's name says; none for another file.
fn parse_region_name(name: &str) -> Option<(i32, (i32, i32))> {
    let stem = name.strip_suffix(".bin")?;
    let mut parts = stem.split('.');
    match parts.next()? {
        "r" => {
            let x = parts.next()?.parse().ok()?;
            let z = parts.next()?.parse().ok()?;
            parts.next().is_none().then_some((SURFACE, (x, z)))
        }
        "c" => {
            let layer = parts.next()?.parse().ok()?;
            let x = parts.next()?.parse().ok()?;
            let z = parts.next()?.parse().ok()?;
            parts.next().is_none().then_some((layer, (x, z)))
        }
        _ => None,
    }
}

fn encode_region(
    cells: &HashMap<(i32, i32, i32), Cell>,
    layer: i32,
    region: (i32, i32),
) -> Vec<u8> {
    let mut keys: Vec<(i32, i32)> = cells
        .keys()
        .filter(|k| k.0 == layer && region_of((k.1, k.2)) == region)
        .map(|k| (k.1, k.2))
        .collect();
    keys.sort_unstable();
    let mut out = Vec::with_capacity(10 + keys.len() * CELL_BYTES);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(keys.len() as u32).to_le_bytes());
    for key in keys {
        let c = &cells[&(layer, key.0, key.1)];
        out.extend_from_slice(&key.0.to_le_bytes());
        out.extend_from_slice(&key.1.to_le_bytes());
        out.extend_from_slice(&c.color);
        for h in c.height {
            out.extend_from_slice(&h.to_le_bytes());
        }
        out.extend_from_slice(&c.depth);
        out.extend_from_slice(&c.glow);
    }
    out
}

fn decode_region(bytes: &[u8]) -> Vec<((i32, i32), Cell)> {
    let mut out = Vec::new();
    if bytes.len() < 10 {
        return out;
    }
    // The first format has no block light: those cells are dark at night until the
    // chunk is loaded again and read afresh.
    let size = if &bytes[..6] == MAGIC {
        CELL_BYTES
    } else if &bytes[..6] == MAGIC_V1 {
        CELL_BYTES_V1
    } else {
        return out;
    };
    let count = u32::from_le_bytes([bytes[6], bytes[7], bytes[8], bytes[9]]) as usize;
    let mut at = 10;
    for _ in 0..count {
        if at + size > bytes.len() {
            break;
        }
        let b = &bytes[at..at + size];
        at += size;
        let cx = i32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        let cz = i32::from_le_bytes([b[4], b[5], b[6], b[7]]);
        let mut cell = Cell {
            color: [0; 256],
            height: [0; 256],
            depth: [0; 256],
            glow: [0; 256],
            glow_mips: [0; GLOW_MIP_BYTES],
            head: 0,
            rgba: [0; PICTURE_BYTES],
            mips: [0; MIP_BYTES],
            revision: u64::MAX,
            scanned: Instant::now(),
        };
        cell.color.copy_from_slice(&b[8..264]);
        for i in 0..256 {
            cell.height[i] = i16::from_le_bytes([b[264 + i * 2], b[265 + i * 2]]);
        }
        cell.depth.copy_from_slice(&b[776..1032]);
        if size == CELL_BYTES {
            cell.glow.copy_from_slice(&b[1032..1288]);
            cell.glow_mips = build_glow_mips(&cell.glow);
        }
        out.push(((cx, cz), cell));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regions_round_trip_through_bytes() {
        let mut cells = HashMap::new();
        let mut cell = Cell {
            color: [7; 256],
            height: [-12; 256],
            depth: [3; 256],
            glow: [0; 256],
            glow_mips: [0; GLOW_MIP_BYTES],
            head: 0,
            rgba: [0; PICTURE_BYTES],
            mips: [0; MIP_BYTES],
            revision: 1,
            scanned: Instant::now(),
        };
        cell.height[5] = 300;
        cell.glow[9] = 14;
        cells.insert((SURFACE, -1, 33), cell.clone());
        cells.insert((SURFACE, 40, 0), cell.clone());
        cells.insert((3, -1, 33), cell);
        let region = region_of((-1, 33));
        assert_eq!(region, (-1, 1));
        let back = decode_region(&encode_region(&cells, SURFACE, region));
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].0, (-1, 33));
        assert_eq!(back[0].1.height[5], 300);
        assert_eq!(back[0].1.color[200], 7);
        assert_eq!(back[0].1.glow[9], 14);
        assert_eq!(decode_region(&encode_region(&cells, 3, region)).len(), 1);
        assert!(decode_region(b"nope").is_empty());
    }

    #[test]
    fn the_first_format_is_still_read() {
        let mut bytes = MAGIC_V1.to_vec();
        bytes.extend_from_slice(&1u32.to_le_bytes());
        let mut cell = vec![0u8; CELL_BYTES_V1];
        cell[0..4].copy_from_slice(&5i32.to_le_bytes());
        cell[4..8].copy_from_slice(&(-2i32).to_le_bytes());
        cell[8 + 7] = 3;
        bytes.extend_from_slice(&cell);
        let back = decode_region(&bytes);
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].0, (5, -2));
        assert_eq!(back[0].1.color[7], 3);
        assert_eq!(back[0].1.glow, [0; 256]);
    }

    #[test]
    fn the_night_dims_the_map_but_not_a_lit_block() {
        let grass = [90, 160, 60, 255];
        assert_eq!(lit(grass, 0, DAY), grass);
        let night = lit(grass, 0, 0);
        assert!(night[1] < 60 && night[1] > 30, "{night:?}");
        assert_eq!(night[3], 255);
        // A torch's own light keeps its colour; the dusk is in between.
        assert_eq!(lit(grass, 15, 0), grass);
        let dim = lit(grass, 7, 0);
        assert!(dim[1] > night[1] && dim[1] < grass[1], "{dim:?}");
        let dusk = lit(grass, 0, DAY / 2);
        assert!(dusk[1] > night[1] && dusk[1] < grass[1]);
        // What was never explored stays as it is.
        assert_eq!(lit(UNEXPLORED, 0, 0), UNEXPLORED);
    }

    #[test]
    fn a_turned_image_has_the_way_you_face_up() {
        let mut map = WorldMap::default();
        let mut cell = Cell {
            color: [0; 256],
            height: [0; 256],
            depth: [0; 256],
            glow: [0; 256],
            glow_mips: [0; GLOW_MIP_BYTES],
            head: 0,
            rgba: [0; PICTURE_BYTES],
            mips: [0; MIP_BYTES],
            revision: 0,
            scanned: Instant::now(),
        };
        // A red block at the cell's block (12, 4), the rest grey.
        for i in 0..256 {
            cell.rgba[i * 4..i * 4 + 4].copy_from_slice(&[100, 100, 100, 255]);
        }
        let red = (4 * 16 + 12) * 4;
        cell.rgba[red..red + 4].copy_from_slice(&[255, 0, 0, 255]);
        map.cells.insert((SURFACE, 0, 0), cell);
        let (w, h) = (9usize, 9usize);
        // Standing on block (8, 8) of the cell, 4 blocks west of the red one... it is east:
        // facing south (yaw 0) east is on the left; facing north (yaw 180) it is on the right.
        let at = |map: &mut WorldMap, yaw: f32| {
            map.composed = None;
            assert!(map.compose(SURFACE, 8.5, 8.5, 1.0, w, h, DAY, Some(yaw)));
            let mut found = None;
            for j in 0..h {
                for i in 0..w {
                    if map.image[(j * w + i) * 4] == 255 {
                        found = Some((i, j));
                    }
                }
            }
            found
        };
        // Red is 4 east and 4 north of the standing block: (12, 4) against (8, 8).
        assert_eq!(at(&mut map, 180.0), Some((8, 0)), "facing north: right and up");
        assert_eq!(at(&mut map, 0.0), Some((0, 8)), "facing south: left and down");
        assert_eq!(at(&mut map, 90.0).map(|p| p.0 <= 8 && p.1 <= 8), Some(true));
    }

    #[test]
    fn glow_halves_to_its_mean() {
        let mut glow = [0u8; 256];
        glow[0] = 15;
        let mips = build_glow_mips(&glow);
        assert_eq!(mips[GLOW_MIP_OFFSET[1]], 4);
        assert_eq!(mips[GLOW_MIP_OFFSET[4]], 1);
    }

    #[test]
    fn region_names_say_their_layer() {
        assert_eq!(region_name(SURFACE, (-1, 2)), "r.-1.2");
        assert_eq!(region_name(-4, (1, -2)), "c.-4.1.-2");
        assert_eq!(parse_region_name("r.-1.2.bin"), Some((SURFACE, (-1, 2))));
        assert_eq!(parse_region_name("c.-4.1.-2.bin"), Some((-4, (1, -2))));
        assert_eq!(parse_region_name("r.1.2.tmp"), None);
        assert_eq!(parse_region_name("notes.bin"), None);
        assert_eq!(cave_layer(-1), -1);
        assert_eq!(cave_layer(70), 4);
        assert_eq!(layer_top(4), 79);
    }
}
