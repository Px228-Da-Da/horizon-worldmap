// What the Key Binds rows of the mods need between the game's calls: whether the page
// is being laid out, and how many pixels the mods' rows add to its list.
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

static INSIDE: AtomicBool = AtomicBool::new(false);
static EXTRA: AtomicU32 = AtomicU32::new(0);

/// The page's layout begins.
pub(crate) fn enter() {
    INSIDE.store(true, Ordering::Relaxed);
}

/// It ends, with the rows the mods added to the list (their height in pixels).
pub(crate) fn leave(extra: f32) {
    INSIDE.store(false, Ordering::Relaxed);
    EXTRA.store(extra.to_bits(), Ordering::Relaxed);
}

/// The height to add to the list's, while the page is being laid out.
pub(crate) fn extra_inside() -> f32 {
    if INSIDE.load(Ordering::Relaxed) {
        f32::from_bits(EXTRA.load(Ordering::Relaxed))
    } else {
        0.0
    }
}
