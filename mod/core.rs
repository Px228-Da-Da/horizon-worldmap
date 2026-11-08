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
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
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
/// Only the chunks this near the player are read again after a block change somewhere in the
/// world (it can change all the time on a busy server); the farther ones are read when new,
/// or from disk, and again when the player comes near.
const REFRESH_RING: i32 = 4;
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
    cells: &HashMap<(i32, i32, i32), Box<Cell>>,
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
fn refresh(cells: &mut HashMap<(i32, i32, i32), Box<Cell>>, id: (i32, i32, i32)) {
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

/// The texels of one size of the cells' pictures (0: a block each, 1: 2 x 2 blocks...) over a
/// rectangle of the world, in one array: what the smooth image is taken from. A texel with no
/// alpha is one that is not explored.
struct Tex {
    span: f64,
    x0: i32,
    z0: i32,
    w: usize,
    h: usize,
    rgba: Vec<[u8; 4]>,
    glow: Vec<u8>,
}

impl Tex {
    /// The texels of `level` that cover blocks x from `xmin` to `xmax` and z from `zmin` to `zmax`.
    fn window(map: &WorldMap, layer: i32, level: usize, xmin: f64, xmax: f64, zmin: f64, zmax: f64) -> Self {
        let span = f64::from(1u32 << level);
        // The texels a bilinear pick at any of these points reads.
        let x0 = (xmin / span - 0.5).floor() as i32;
        let x1 = (xmax / span - 0.5).floor() as i32 + 1;
        let z0 = (zmin / span - 0.5).floor() as i32;
        let z1 = (zmax / span - 0.5).floor() as i32 + 1;
        let (w, h) = ((x1 - x0 + 1) as usize, (z1 - z0 + 1) as usize);
        let mut rgba = vec![[0u8; 4]; w * h];
        let mut glow = vec![0u8; w * h];
        let size = 16i32 >> level;
        for cz in z0.div_euclid(size)..=z1.div_euclid(size) {
            for cx in x0.div_euclid(size)..=x1.div_euclid(size) {
                let Some(cell) = map.cells.get(&(layer, cx, cz)) else {
                    continue;
                };
                // The part of this cell that is in the rectangle.
                let (lx0, lx1) = ((x0 - cx * size).max(0), (x1 - cx * size).min(size - 1));
                let (lz0, lz1) = ((z0 - cz * size).max(0), (z1 - cz * size).min(size - 1));
                for lz in lz0..=lz1 {
                    let row = ((cz * size + lz - z0) as usize) * w + (cx * size + lx0 - x0) as usize;
                    let count = (lx1 - lx0 + 1) as usize;
                    let lzu = lz as usize;
                    if level == 0 {
                        let from = lzu * 16 + lx0 as usize;
                        for k in 0..count {
                            let at = (from + k) * 4;
                            rgba[row + k] = [cell.rgba[at], cell.rgba[at + 1], cell.rgba[at + 2], cell.rgba[at + 3]];
                        }
                        glow[row..row + count].copy_from_slice(&cell.glow[from..from + count]);
                    } else {
                        let side = size as usize;
                        let from = lzu * side + lx0 as usize;
                        for k in 0..count {
                            let at = MIP_OFFSET[level] + (from + k) * 4;
                            rgba[row + k] = [cell.mips[at], cell.mips[at + 1], cell.mips[at + 2], cell.mips[at + 3]];
                        }
                        let g = GLOW_MIP_OFFSET[level] + from;
                        glow[row..row + count].copy_from_slice(&cell.glow_mips[g..g + count]);
                    }
                }
            }
        }
        Self { span, x0, z0, w, h, rgba, glow }
    }

    /// Bilinear: the four texels round the point (block coordinates), by how near they are; the
    /// ones that are not explored are left out. Its colour and block light.
    #[inline]
    fn bilinear(&self, xf: f64, zf: f64) -> Option<([f32; 3], f32)> {
        let (u, v) = (xf / self.span - 0.5, zf / self.span - 0.5);
        let (i, j) = (u.floor(), v.floor());
        let (fu, fv) = ((u - i) as f32, (v - j) as f32);
        let (i, j) = ((i as i32 - self.x0) as usize, (j as i32 - self.z0) as usize);
        if i + 1 >= self.w || j + 1 >= self.h {
            return None;
        }
        let at = j * self.w + i;
        let mut sum = [0.0f32; 3];
        let (mut glow, mut weight) = (0.0f32, 0.0f32);
        for (k, wt) in [
            (at, (1.0 - fu) * (1.0 - fv)),
            (at + 1, fu * (1.0 - fv)),
            (at + self.w, (1.0 - fu) * fv),
            (at + self.w + 1, fu * fv),
        ] {
            let c = self.rgba[k];
            if wt > 0.0 && c[3] != 0 {
                sum[0] += wt * f32::from(c[0]);
                sum[1] += wt * f32::from(c[1]);
                sum[2] += wt * f32::from(c[2]);
                glow += wt * f32::from(self.glow[k]);
                weight += wt;
            }
        }
        (weight > 0.0).then(|| ([sum[0] / weight, sum[1] / weight, sum[2] / weight], glow / weight))
    }
}

#[derive(Default)]
pub struct WorldMap {
    /// By layer, chunk x and chunk z.
    cells: HashMap<(i32, i32, i32), Box<Cell>>,
    palette: Option<Palette>,
    /// Regions with cells not yet saved: layer, region x and z.
    dirty: HashSet<(i32, i32, i32)>,
    /// A cell changed since the image was composed.
    changed: bool,
    composed: Option<(i32, f64, f64, f32, usize, usize, u8, i32)>,
    /// Regions a writing thread could not write, to be written again; and the regions it is
    /// writing now.
    failed: Arc<Mutex<Vec<(i32, i32, i32)>>>,
    writing: Arc<Mutex<HashSet<(i32, i32, i32)>>>,
    /// Region files still to be read (the next is the last), the cells of the one being read,
    /// its layer, and how many cells have been read.
    to_load: Vec<(i64, i32, PathBuf)>,
    /// How many cells each layer has (counting them would go through every cell of the map).
    counts: HashMap<i32, usize>,
    reading: Option<std::vec::IntoIter<((i32, i32), Cell)>>,
    reading_layer: i32,
    read: usize,
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
        self.counts.get(&layer).copied().unwrap_or(0)
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
                            // Read from disk: once. Changed since: only near the player.
                            c.revision == u64::MAX
                                || (ring <= REFRESH_RING
                                    && c.revision != world.revision
                                    && c.scanned.elapsed() >= RESCAN_AFTER)
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
                    let fresh = self.cells.insert(
                        id,
                        Box::new(Cell {
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
                        }),
                    );
                    if fresh.is_none() {
                        *self.counts.entry(layer).or_default() += 1;
                    }
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
        let (sin, cos) = f64::from(yaw).to_radians().sin_cos();
        // Forward, in the world's x and z; right of it is (-fz, fx).
        self.render_smooth(layer, cx, cz, bpp, w, h, light, Some((-sin, cos)), img);
    }

    /// The image with every pixel taken smoothly: bilinear in the cells' pictures, and between
    /// two sizes of them (the next smaller one's) by how far the map is zoomed out. Nothing
    /// jumps when it is zoomed or turned, and a dithered forest does not shimmer. `forward` is
    /// the way that is up when the map is turned (else north is).
    #[allow(clippy::too_many_arguments)]
    fn render_smooth(
        &self,
        layer: i32,
        cx: f64,
        cz: f64,
        bpp: f32,
        w: usize,
        h: usize,
        light: u8,
        forward: Option<(f64, f64)>,
        img: &mut Vec<u8>,
    ) {
        // Zoomed out: the pictures of the cells are laid on the pixels (the work is that of the
        // texels there are) rather than the pixels looked up in a window of texels (the work is
        // that of the pixels, and the window grows with the square of the zoom).
        if forward.is_none() && bpp >= 2.0 {
            let level = (bpp.log2().floor() as usize).min(4);
            let side = (16usize >> level) as f64;
            let in_view = ((w as f64 * f64::from(bpp) / 16.0 + 2.0) * (h as f64 * f64::from(bpp) / 16.0 + 2.0))
                .min(self.cells.len() as f64);
            if in_view * side * side < 1.5 * (w * h) as f64 {
                self.render_splat(layer, level, cx, cz, bpp, w, h, light, img);
                return;
            }
        }
        img.resize(w * h * 4, 0);
        let bpp = f64::from(bpp);
        let (hw, hh) = (w as f64 / 2.0, h as f64 / 2.0);
        // Which two sizes of the picture, and how much of the second.
        let t = bpp.log2().clamp(0.0, 4.0);
        let level = (t.floor() as usize).min(4);
        let frac = (t - level as f64) as f32;
        let blend = level < 4 && frac >= 0.02;
        // The part of the world the image shows (all of a circle round it when it is turned).
        let (rx, rz) = match forward {
            Some(_) => {
                let r = hw.hypot(hh) * bpp;
                (r, r)
            }
            None => (hw * bpp, hh * bpp),
        };
        let (x0, x1, z0, z1) = (cx - rx - 1.0, cx + rx + 1.0, cz - rz - 1.0, cz + rz + 1.0);
        let near = Tex::window(self, layer, level, x0, x1, z0, z1);
        let far = blend.then(|| Tex::window(self, layer, level + 1, x0, x1, z0, z1));
        // One row of the image into `out` (4 bytes a pixel).
        let pixel_row = |j: usize, out: &mut [u8]| {
            let v = (j as f64 + 0.5 - hh) * bpp;
            for (i, px) in out.chunks_exact_mut(4).enumerate() {
                let u = (i as f64 + 0.5 - hw) * bpp;
                let (xf, zf) = match forward {
                    Some((fx, fz)) => (cx - fz * u - fx * v, cz + fx * u - fz * v),
                    None => (cx + u, cz + v),
                };
                let a = near.bilinear(xf, zf);
                let picked = match &far {
                    Some(far) => match (a, far.bilinear(xf, zf)) {
                        (Some(a), Some(b)) => Some((
                            [
                                a.0[0] + (b.0[0] - a.0[0]) * frac,
                                a.0[1] + (b.0[1] - a.0[1]) * frac,
                                a.0[2] + (b.0[2] - a.0[2]) * frac,
                            ],
                            a.1 + (b.1 - a.1) * frac,
                        )),
                        (only, None) => only,
                        (None, only) => only,
                    },
                    None => a,
                };
                let rgba = match picked {
                    Some((c, glow)) => lit(
                        [
                            c[0].round().clamp(0.0, 255.0) as u8,
                            c[1].round().clamp(0.0, 255.0) as u8,
                            c[2].round().clamp(0.0, 255.0) as u8,
                            255,
                        ],
                        glow.round().clamp(0.0, 15.0) as u8,
                        light,
                    ),
                    None => UNEXPLORED,
                };
                px.copy_from_slice(&rgba);
            }
        };
        // A big image is drawn by several threads, a band of rows each (a small one, the
        // minimap's, is not worth the start-up).
        let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).min(4);
        if threads > 1 && w * h >= 40_000 {
            let band = h.div_ceil(threads);
            let row = &pixel_row;
            std::thread::scope(|scope| {
                for (n, chunk) in img.chunks_mut(band * w * 4).enumerate() {
                    scope.spawn(move || {
                        for (k, out) in chunk.chunks_exact_mut(w * 4).enumerate() {
                            row(n * band + k, out);
                        }
                    });
                }
            });
        } else {
            for (j, out) in img.chunks_exact_mut(w * 4).enumerate() {
                pixel_row(j, out);
            }
        }
    }

    /// The image when a texel of picture size `level` (0: a block, 1: 2 x 2 blocks... 4: a cell)
    /// is no bigger than a pixel: each explored texel puts its colour on the four pixels round
    /// its middle (by how near), and a pixel is the mean of what it got. The work is that of the
    /// texels there are, however far out the map is zoomed, and nothing shimmers when it moves.
    #[allow(clippy::too_many_arguments)]
    fn render_splat(
        &self,
        layer: i32,
        level: usize,
        cx: f64,
        cz: f64,
        bpp: f32,
        w: usize,
        h: usize,
        light: u8,
        img: &mut Vec<u8>,
    ) {
        img.resize(w * h * 4, 0);
        let bpp = f64::from(bpp);
        let (hw, hh) = (w as f64 / 2.0, h as f64 / 2.0);
        let side = 16usize >> level;
        let span = f64::from(1u32 << level);
        let step = span / bpp;
        // Red, green, blue, block light and weight of each pixel.
        let mut sums = vec![[0.0f32; 5]; w * h];
        for (&(l, kx, kz), cell) in &self.cells {
            if l != layer {
                continue;
            }
            // Where the cell's first texel is, in pixels, and the cell's reach.
            let fx0 = (f64::from(kx) * 16.0 + span / 2.0 - cx) / bpp + hw - 0.5;
            let fz0 = (f64::from(kz) * 16.0 + span / 2.0 - cz) / bpp + hh - 0.5;
            let reach = side as f64 * step;
            if fx0 + reach < -1.0 || fz0 + reach < -1.0 || fx0 >= w as f64 || fz0 >= h as f64 {
                continue;
            }
            for tz in 0..side {
                let fz = fz0 + tz as f64 * step;
                if fz < -1.0 || fz >= h as f64 {
                    continue;
                }
                let (j0, vz) = (fz.floor(), (fz - fz.floor()) as f32);
                for tx in 0..side {
                    let fx = fx0 + tx as f64 * step;
                    if fx < -1.0 || fx >= w as f64 {
                        continue;
                    }
                    let (rgba, glow) = if level == 0 {
                        let at = tz * 16 + tx;
                        ([cell.rgba[at * 4], cell.rgba[at * 4 + 1], cell.rgba[at * 4 + 2], cell.rgba[at * 4 + 3]], cell.glow[at])
                    } else {
                        let at = MIP_OFFSET[level] + (tz * side + tx) * 4;
                        (
                            [cell.mips[at], cell.mips[at + 1], cell.mips[at + 2], cell.mips[at + 3]],
                            cell.glow_mips[GLOW_MIP_OFFSET[level] + tz * side + tx],
                        )
                    };
                    if rgba[3] == 0 {
                        continue;
                    }
                    let (i0, ux) = (fx.floor(), (fx - fx.floor()) as f32);
                    let colour = [f32::from(rgba[0]), f32::from(rgba[1]), f32::from(rgba[2]), f32::from(glow)];
                    for (di, dj, wt) in [
                        (0, 0, (1.0 - ux) * (1.0 - vz)),
                        (1, 0, ux * (1.0 - vz)),
                        (0, 1, (1.0 - ux) * vz),
                        (1, 1, ux * vz),
                    ] {
                        let (i, j) = (i0 as i64 + di, j0 as i64 + dj);
                        if i < 0 || j < 0 || i >= w as i64 || j >= h as i64 {
                            continue;
                        }
                        let s = &mut sums[j as usize * w + i as usize];
                        for k in 0..4 {
                            s[k] += wt * colour[k];
                        }
                        s[4] += wt;
                    }
                }
            }
        }
        // A pixel that got less than a fifth of one texel is not explored.
        let need = 0.2 * (step as f32).powi(2).min(1.0);
        for (px, s) in img.chunks_exact_mut(4).zip(&sums) {
            let rgba = if s[4] >= need.max(0.0001) {
                lit(
                    [
                        (s[0] / s[4]).round().clamp(0.0, 255.0) as u8,
                        (s[1] / s[4]).round().clamp(0.0, 255.0) as u8,
                        (s[2] / s[4]).round().clamp(0.0, 255.0) as u8,
                        255,
                    ],
                    (s[3] / s[4]).round().clamp(0.0, 15.0) as u8,
                    light,
                )
            } else {
                UNEXPLORED
            };
            px.copy_from_slice(&rgba);
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
        // More than a block to a pixel: smoothly (a sharp pick would jump between sizes and shimmer).
        if bpp > 1.0 {
            self.render_smooth(layer, cx, cz, bpp, w, h, light, None, img);
            return;
        }
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
                            let c = cells.get(&(layer, key.0, key.1)).map(|b| &**b);
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
                        let c = cells.get(&(layer, key.0, key.1)).map(|b| &**b);
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

    /// Get ready to read the region files of `dir`, the ones nearest chunk `near` first (cells
    /// already in memory win). `load_step` reads them, a little in each frame.
    pub fn begin_load(&mut self, dir: &Path, near: (i32, i32)) {
        self.to_load.clear();
        self.reading = None;
        self.read = 0;
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let Some((layer, (rx, rz))) = parse_region_name(&name) else {
                continue;
            };
            let (mx, mz) = (rx * REGION + REGION / 2 - near.0, rz * REGION + REGION / 2 - near.1);
            let far = i64::from(mx) * i64::from(mx) + i64::from(mz) * i64::from(mz);
            self.to_load.push((far, layer, entry.path()));
        }
        // The farthest first, so that the nearest is the last.
        self.to_load.sort_unstable_by(|a, b| b.0.cmp(&a.0));
    }

    /// Read region files for at most `budget` (a cell at a time, so the frame is not held up by
    /// a big map), each cell shaded as it comes. True while there is more to read.
    pub fn load_step(&mut self, budget: Duration) -> bool {
        let started = Instant::now();
        loop {
            if self.reading.is_none() {
                let Some((_, layer, path)) = self.to_load.pop() else {
                    return false;
                };
                let Ok(bytes) = fs::read(&path) else {
                    continue;
                };
                self.reading_layer = layer;
                self.reading = Some(decode_region(&bytes).into_iter());
            }
            let layer = self.reading_layer;
            let mut done = 0u32;
            while let Some((key, cell)) = self.reading.as_mut().and_then(Iterator::next) {
                let id = (layer, key.0, key.1);
                if let std::collections::hash_map::Entry::Vacant(v) = self.cells.entry(id) {
                    v.insert(Box::new(cell));
                    *self.counts.entry(layer).or_default() += 1;
                    refresh(&mut self.cells, id);
                    self.read += 1;
                    self.changed = true;
                }
                done += 1;
                if done % 16 == 0 && started.elapsed() >= budget {
                    return true;
                }
            }
            self.reading = None;
            if started.elapsed() >= budget {
                return !self.to_load.is_empty();
            }
        }
    }

    /// How many region files are still to be read, and how many cells have been so far.
    pub fn loading(&self) -> (usize, usize) {
        (self.to_load.len() + usize::from(self.reading.is_some()), self.read)
    }

    /// Write the regions that changed, at most `limit` of them (the rest next time). With
    /// `background` another thread writes the files, so a slow disk does not stall the frame;
    /// a region it could not write is queued again. Without it every region is written before
    /// this returns (and after those a thread is still writing). Returns how many were taken.
    pub fn save_dirty(&mut self, dir: &Path, limit: usize, background: bool) -> io::Result<usize> {
        if let Ok(mut failed) = self.failed.lock() {
            self.dirty.extend(failed.drain(..));
        }
        if !background {
            // Wait for the writes in progress, so a newer file is not overwritten by an older.
            for _ in 0..400 {
                if self.writing.lock().is_ok_and(|w| w.is_empty()) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        if self.dirty.is_empty() {
            return Ok(0);
        }
        fs::create_dir_all(dir)?;
        let busy: HashSet<(i32, i32, i32)> = self.writing.lock().map(|w| w.clone()).unwrap_or_default();
        let regions: Vec<(i32, i32, i32)> = self
            .dirty
            .iter()
            .copied()
            .filter(|r| !busy.contains(r))
            .take(limit)
            .collect();
        for r in &regions {
            self.dirty.remove(r);
        }
        let jobs: Vec<((i32, i32, i32), PathBuf, PathBuf, Vec<u8>)> = regions
            .into_iter()
            .map(|region| {
                let stem = region_name(region.0, (region.1, region.2));
                (
                    region,
                    dir.join(format!("{stem}.bin")),
                    dir.join(format!("{stem}.tmp")),
                    encode_region(&self.cells, region.0, (region.1, region.2)),
                )
            })
            .collect();
        let taken = jobs.len();
        if let Ok(mut w) = self.writing.lock() {
            w.extend(jobs.iter().map(|j| j.0));
        }
        let failed = Arc::clone(&self.failed);
        let writing = Arc::clone(&self.writing);
        let write = move |jobs: Vec<((i32, i32, i32), PathBuf, PathBuf, Vec<u8>)>| {
            let mut first: Option<io::Error> = None;
            for (region, path, temp, bytes) in jobs {
                if let Err(e) = fs::write(&temp, &bytes).and_then(|()| fs::rename(&temp, &path)) {
                    if let Ok(mut f) = failed.lock() {
                        f.push(region);
                    }
                    first.get_or_insert(e);
                }
                if let Ok(mut w) = writing.lock() {
                    w.remove(&region);
                }
            }
            first
        };
        if background {
            std::thread::spawn(move || {
                if let Some(e) = write(jobs) {
                    log::warn!("World Map: could not save the map: {e}");
                }
            });
            Ok(taken)
        } else {
            match write(jobs) {
                Some(e) => Err(e),
                None => Ok(taken),
            }
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
    cells: &HashMap<(i32, i32, i32), Box<Cell>>,
    layer: i32,
    region: (i32, i32),
) -> Vec<u8> {
    // The region's 32 x 32 places are looked at, not all the cells of the map.
    let mut keys: Vec<(i32, i32)> = Vec::new();
    for dz in 0..REGION {
        for dx in 0..REGION {
            let key = (region.0 * REGION + dx, region.1 * REGION + dz);
            if cells.contains_key(&(layer, key.0, key.1)) {
                keys.push(key);
            }
        }
    }
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
        cells.insert((SURFACE, -1, 33), Box::new(cell.clone()));
        cells.insert((SURFACE, 40, 0), Box::new(cell.clone()));
        cells.insert((3, -1, 33), Box::new(cell));
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
    fn regions_are_written_now_or_by_a_thread_a_few_at_a_time() {
        let mut map = WorldMap::default();
        let cell = Cell {
            color: [7; 256],
            height: [60; 256],
            depth: [0; 256],
            glow: [3; 256],
            glow_mips: [0; GLOW_MIP_BYTES],
            head: 0,
            rgba: [0; PICTURE_BYTES],
            mips: [0; MIP_BYTES],
            revision: 1,
            scanned: Instant::now(),
        };
        // Three regions of the surface, a chunk each.
        for x in [0, 40, 80] {
            map.cells.insert((SURFACE, x, 0), Box::new(cell.clone()));
            map.dirty.insert((SURFACE, x / REGION, 0));
        }
        let dir = std::env::temp_dir().join(format!("worldmap-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        // One, by a thread: the other two wait.
        assert_eq!(map.save_dirty(&dir, 1, true).unwrap(), 1);
        assert_eq!(map.dirty.len(), 2);
        // Everything, now: it waits for the thread, and every file is there when it returns.
        assert_eq!(map.save_dirty(&dir, usize::MAX, false).unwrap(), 2);
        assert!(map.dirty.is_empty());
        let mut files: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        files.sort();
        assert_eq!(files, ["r.0.0.bin", "r.1.0.bin", "r.2.0.bin"]);
        // What was written is read back.
        let mut back = WorldMap::default();
        back.begin_load(&dir, (0, 0));
        while back.load_step(Duration::from_secs(5)) {}
        assert_eq!(back.loading().1, 3);
        assert_eq!(back.column(SURFACE, 40 * 16, 0).map(|c| c.1), Some(60));
        let _ = fs::remove_dir_all(&dir);
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
        map.cells.insert((SURFACE, 0, 0), Box::new(cell));
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
    fn zooming_out_does_not_jump_between_sizes_of_the_picture() {
        // A cell of a pattern as fine as it can be: a block light, a block dark.
        let mut cell = Cell {
            color: [7; 256],
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
        for i in 0..256 {
            let v = if (i % 16 + i / 16) % 2 == 0 { 230 } else { 30 };
            cell.rgba[i * 4..i * 4 + 4].copy_from_slice(&[v, v, v, 255]);
        }
        cell.mips = build_mips(&cell.rgba);
        let mut map = WorldMap::default();
        for x in -1..=1 {
            for z in -1..=1 {
                map.cells.insert((SURFACE, x, z), Box::new(cell.clone()));
            }
        }
        let mean_diff = |a: &[u8], b: &[u8]| {
            let n = a.len().min(b.len());
            a[..n].iter().zip(&b[..n]).map(|(x, y)| (i32::from(*x) - i32::from(*y)).unsigned_abs() as u64).sum::<u64>() as f64
                / n as f64
        };
        let (mut a, mut b) = (Vec::new(), Vec::new());
        // Just under and just over two blocks to a pixel: a sharp pick would flip from the
        // pattern to its mean here.
        map.render(SURFACE, 8.0, 8.0, 1.97, 24, 24, DAY, &mut a);
        map.render(SURFACE, 8.0, 8.0, 2.03, 24, 24, DAY, &mut b);
        assert!(mean_diff(&a, &b) < 12.0, "{}", mean_diff(&a, &b));
        // And it is not blurred away for nothing: at a block to a pixel the pattern is there.
        map.render(SURFACE, 8.0, 8.0, 1.0, 24, 24, DAY, &mut a);
        let (lo, hi) = (a.iter().step_by(4).min().copied().unwrap(), a.iter().step_by(4).max().copied().unwrap());
        assert!(hi - lo > 150, "{lo} {hi}");
        // Turned a little, a pixel does not change much: the image does not crawl.
        map.render_turned(SURFACE, 8.0, 8.0, 2.0, 32, 32, DAY, 40.0, &mut a);
        map.render_turned(SURFACE, 8.0, 8.0, 2.0, 32, 32, DAY, 41.0, &mut b);
        assert!(mean_diff(&a, &b) < 14.0, "{}", mean_diff(&a, &b));
    }

    #[test]
    fn far_out_the_map_is_drawn_from_its_cells() {
        let mut cell = Cell {
            color: [7; 256],
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
        for i in 0..256 {
            cell.rgba[i * 4..i * 4 + 4].copy_from_slice(&[90, 160, 40, 255]);
        }
        cell.mips = build_mips(&cell.rgba);
        let mut map = WorldMap::default();
        // A square of 20 x 20 cells, 320 blocks, round the middle.
        for x in -10..10 {
            for z in -10..10 {
                map.cells.insert((SURFACE, x, z), Box::new(cell.clone()));
            }
        }
        let (mut a, mut b) = (Vec::new(), Vec::new());
        // 40 blocks a pixel: the square is 8 pixels wide, all of it its colour, round it nothing.
        map.render(SURFACE, 0.0, 0.0, 40.0, 24, 24, DAY, &mut a);
        let px = |img: &[u8], x: usize, y: usize| [img[(y * 24 + x) * 4], img[(y * 24 + x) * 4 + 1], img[(y * 24 + x) * 4 + 2]];
        let mid = px(&a, 12, 12);
        assert!(mid[0].abs_diff(90) < 3 && mid[1].abs_diff(160) < 3, "{mid:?}");
        assert_eq!(&a[0..4], &UNEXPLORED);
        // Nothing jumps where the far way of drawing takes over from the near one.
        map.render(SURFACE, 0.0, 0.0, 15.9, 24, 24, DAY, &mut a);
        map.render(SURFACE, 0.0, 0.0, 16.1, 24, 24, DAY, &mut b);
        let n = a.len();
        let diff: f64 = a.iter().zip(&b).map(|(x, y)| f64::from(x.abs_diff(*y))).sum::<f64>() / n as f64;
        assert!(diff < 25.0, "{diff}");
        // And a lone cell, far out, is still seen.
        let mut lone = WorldMap::default();
        lone.cells.insert((SURFACE, 0, 0), Box::new(cell));
        lone.render(SURFACE, 8.0, 8.0, 64.0, 24, 24, DAY, &mut a);
        assert!(a.chunks_exact(4).any(|p| p[3] == 255 && p[0] > 60), "lone cell lost");
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
