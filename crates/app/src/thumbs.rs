//! Album and playlist art: fetched and decoded off the UI thread, cached as
//! textures.
//!
//! The cache lives in the egui `Context` data, so any painter can ask for
//! art without the app wiring anything up. `get` picks the smallest
//! thumbnail that covers the slot and returns `None` until it is ready. Two
//! worker threads fetch (or read the disk cache), decode and downscale; the
//! UI thread only uploads finished pixels and asks for one repaint per
//! image. Failures are remembered for the session and never retried, so a
//! machine without network does not repaint in a loop.

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

use egui::{ColorImage, Context, Id, TextureHandle, TextureId, TextureOptions};
use model::Thumbnail;

/// Most textures kept at once.
const MAX_TEXTURES: usize = 200;
/// Texture memory budget. llvmpipe keeps textures in system RAM, so this
/// counts against the app's RSS.
const MAX_TEXTURE_BYTES: usize = 12 << 20;
/// Queued (not yet started) requests. The oldest are dropped first.
const MAX_QUEUED: usize = 48;
/// A queued request is dropped when no frame has asked for it in this
/// many passes, which means its row scrolled away.
const STALE_PASSES: u64 = 2;
const WORKERS: usize = 2;
const MAX_BODY: u64 = 2 << 20;
const MAX_DIM: u32 = 2048;

/// Texture id for the art at `size` points square, if it is ready.
/// Requests it otherwise. `size` is in points; the pixel size follows the
/// display scale.
pub fn get(ctx: &Context, thumbs: &[Thumbnail], size: f32) -> Option<TextureId> {
    // Read everything from `ctx` before taking our own locks: egui's
    // context lock must never be taken while one of ours is held.
    let px = (size * ctx.pixels_per_point()).ceil().max(1.0) as u32;
    let thumb = pick(thumbs, px)?;
    let pass = ctx.cumulative_pass_nr();
    let cache = ctx.data_mut(|d| {
        d.get_temp_mut_or_insert_with(Id::NULL.with("rustle-thumbs"), || Cache::start(ctx.clone()))
            .clone()
    });
    cache.upload_done(ctx);
    let key = key(&thumb.url, px);
    if let Some(tex) = lock(&cache.textures).get(key) {
        return Some(tex.id());
    }
    let mut work = lock(&cache.shared.work);
    if work.failed.contains(&key) {
        return None;
    }
    if work.queue.request(
        Job {
            key,
            url: thumb.url.clone(),
            px,
        },
        pass,
    ) {
        cache.shared.wake.notify_one();
    }
    None
}

/// Computes the art accent color (see `art_color`) for each track in a
/// low-priority background pass and hands results to `sink` in batches.
/// Never blocks the caller and keeps no images. The backend calls this on
/// `Effect::NeedArtColors` with a sink that sends `Event::ArtColors`.
#[allow(dead_code)] // b3-art; called by b3-wiring
pub fn request_colors<F>(_tracks: Vec<Arc<model::Track>>, _sink: F)
where
    F: Fn(Vec<(model::TrackId, [u8; 3])>) + Send + Sync + 'static,
{
    // b3-art
}

/// The smallest thumbnail whose short side covers `px`, else the largest.
/// Entries with an unknown size (0) are used only when nothing else is.
pub fn pick(thumbs: &[Thumbnail], px: u32) -> Option<&Thumbnail> {
    let side = |t: &Thumbnail| t.width.min(t.height);
    thumbs
        .iter()
        .filter(|t| side(t) >= px)
        .min_by_key(|t| side(t))
        .or_else(|| thumbs.iter().max_by_key(|t| side(t)))
}

/// Cache key for one URL drawn at one pixel size.
fn key(url: &str, px: u32) -> u64 {
    let mut h = fnv1a(FNV_OFFSET, url.as_bytes());
    h = fnv1a(h, &px.to_le_bytes());
    h
}

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;

/// FNV-1a 64. Stable across Rust versions, unlike `DefaultHasher`, so disk
/// cache names survive toolchain upgrades.
fn fnv1a(mut h: u64, bytes: &[u8]) -> u64 {
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// Where the downloaded bytes for `url` are cached under `dir`.
fn cache_path(dir: &Path, url: &str) -> PathBuf {
    dir.join(format!("{:016x}", fnv1a(FNV_OFFSET, url.as_bytes())))
}

fn cache_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
    Some(base.join("rustle").join("thumbs"))
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

struct Cache {
    shared: Arc<Shared>,
    textures: Mutex<Lru<TextureHandle>>,
}

impl Cache {
    fn start(ctx: Context) -> Arc<Self> {
        let shared = Arc::new(Shared {
            work: Mutex::new(Work {
                queue: Queue::new(MAX_QUEUED),
                done: Vec::new(),
                failed: HashSet::new(),
            }),
            wake: Condvar::new(),
            ctx,
        });
        let agent = ureq::Agent::new_with_config(
            ureq::Agent::config_builder()
                .timeout_connect(Some(Duration::from_secs(10)))
                .timeout_global(Some(Duration::from_secs(20)))
                // ureq's 128 KB defaults cost about 6 MB of peak RSS across
                // the two workers; thumbnail responses need far less.
                .input_buffer_size(16 << 10)
                .output_buffer_size(4 << 10)
                .build(),
        );
        let dir = cache_dir();
        for i in 0..WORKERS {
            let shared = Arc::clone(&shared);
            let agent = agent.clone();
            let dir = dir.clone();
            let spawned = std::thread::Builder::new()
                .name(format!("rustle-thumbs-{i}"))
                .spawn(move || worker(&shared, &agent, dir.as_deref()));
            if let Err(e) = spawned {
                eprintln!("rustle: could not start thumbnail worker: {e}");
            }
        }
        Arc::new(Self {
            shared,
            textures: Mutex::new(Lru::new(MAX_TEXTURES, MAX_TEXTURE_BYTES)),
        })
    }

    /// Uploads images the workers finished since the last call.
    fn upload_done(&self, ctx: &Context) {
        let done = {
            let mut work = lock(&self.shared.work);
            if work.done.is_empty() {
                return;
            }
            std::mem::take(&mut work.done)
        };
        let uploaded: Vec<_> = done
            .into_iter()
            .map(|(key, image)| {
                let bytes = image.width() * image.height() * 4;
                let name = format!("thumb-{key:016x}");
                (
                    key,
                    ctx.load_texture(name, image, TextureOptions::LINEAR),
                    bytes,
                )
            })
            .collect();
        let mut textures = lock(&self.textures);
        for (key, tex, bytes) in uploaded {
            // Dropping evicted handles frees their textures.
            drop(textures.insert(key, tex, bytes));
        }
    }
}

struct Shared {
    work: Mutex<Work>,
    wake: Condvar,
    ctx: Context,
}

struct Work {
    queue: Queue,
    done: Vec<(u64, ColorImage)>,
    failed: HashSet<u64>,
}

fn worker(shared: &Shared, agent: &ureq::Agent, dir: Option<&Path>) {
    loop {
        let job = {
            let mut work = lock(&shared.work);
            loop {
                if work.queue.is_empty() {
                    work = shared.wake.wait(work).unwrap_or_else(|e| e.into_inner());
                    continue;
                }
                // Release our lock before reading the context's pass number.
                drop(work);
                let pass = shared.ctx.cumulative_pass_nr();
                work = lock(&shared.work);
                if let Some(job) = work.queue.pop(pass) {
                    break job;
                }
            }
        };
        let image = load(agent, dir, &job.url, job.px);
        // Hand the freed download, TLS and decode buffers back to the OS.
        // mimalloc purges freed pages only on later allocations from the
        // same thread, so without this each worker keeps them. Measured
        // under Xvfb: peak RSS 160 MB without it, 146 MB with it.
        // SAFETY: mi_collect has no preconditions.
        unsafe { libmimalloc_sys::mi_collect(true) };
        let ok = image.is_some();
        {
            let mut work = lock(&shared.work);
            work.queue.finish(job.key);
            match image {
                Some(img) => work.done.push((job.key, img)),
                None => {
                    work.failed.insert(job.key);
                }
            }
        }
        if ok {
            shared.ctx.request_repaint();
        }
    }
}

/// Fetches (or reads from disk), decodes and downscales one thumbnail.
fn load(agent: &ureq::Agent, dir: Option<&Path>, url: &str, px: u32) -> Option<ColorImage> {
    let path = dir.map(|d| cache_path(d, url));
    let cached = path.as_ref().and_then(|p| std::fs::read(p).ok());
    let bytes = match cached {
        Some(b) => b,
        None => {
            let b = fetch(agent, url)?;
            if let (Some(dir), Some(path)) = (dir, &path) {
                store(dir, path, &b);
            }
            b
        }
    };
    decode(&bytes, px)
}

fn fetch(agent: &ureq::Agent, url: &str) -> Option<Vec<u8>> {
    let mut resp = agent.get(url).call().ok()?;
    resp.body_mut()
        .with_config()
        .limit(MAX_BODY)
        .read_to_vec()
        .ok()
}

/// Writes through a temp file so a crash never leaves a half file behind.
fn store(dir: &Path, path: &Path, bytes: &[u8]) {
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let tmp = path.with_extension(format!("tmp{:?}", std::thread::current().id()));
    if std::fs::write(&tmp, bytes).is_ok() && std::fs::rename(&tmp, path).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

/// Decodes and center-crops to a square of at most `px` pixels.
fn decode(bytes: &[u8], px: u32) -> Option<ColorImage> {
    let mut reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_DIM);
    limits.max_image_height = Some(MAX_DIM);
    limits.max_alloc = Some(32 << 20);
    reader.limits(limits);
    let img = reader.decode().ok()?;
    let side = px.min(img.width()).min(img.height()).max(1);
    let rgba = img
        .resize_to_fill(side, side, image::imageops::FilterType::Triangle)
        .into_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    Some(ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()))
}

struct Job {
    key: u64,
    url: String,
    px: u32,
}

/// Pending requests, newest first, bounded. A key stays "wanted" from its
/// first request until a worker finishes it or it is dropped, so each
/// image is requested once.
struct Queue {
    jobs: VecDeque<Job>,
    /// Key to the last pass that asked for it. Holds queued and in-flight keys.
    wanted: HashMap<u64, u64>,
    cap: usize,
}

impl Queue {
    fn new(cap: usize) -> Self {
        Self {
            jobs: VecDeque::new(),
            wanted: HashMap::new(),
            cap,
        }
    }

    fn is_empty(&self) -> bool {
        self.jobs.is_empty()
    }

    /// Queues `job`, or marks it still wanted if already pending.
    /// Returns true when a new job was queued.
    fn request(&mut self, job: Job, pass: u64) -> bool {
        if let Some(p) = self.wanted.get_mut(&job.key) {
            *p = pass;
            return false;
        }
        self.wanted.insert(job.key, pass);
        self.jobs.push_back(job);
        if self.jobs.len() > self.cap
            && let Some(old) = self.jobs.pop_front()
        {
            self.wanted.remove(&old.key);
        }
        true
    }

    /// The newest job still wanted at `pass`. Stale jobs are dropped, so
    /// they can be requested again if their row comes back.
    fn pop(&mut self, pass: u64) -> Option<Job> {
        while let Some(job) = self.jobs.pop_back() {
            let last = self.wanted.get(&job.key).copied().unwrap_or(0);
            if last + STALE_PASSES >= pass {
                return Some(job);
            }
            self.wanted.remove(&job.key);
        }
        None
    }

    fn finish(&mut self, key: u64) {
        self.wanted.remove(&key);
    }
}

/// Least-recently-used map capped by entry count and by bytes.
struct Lru<V> {
    map: HashMap<u64, (V, usize, u64)>,
    tick: u64,
    bytes: usize,
    max_len: usize,
    max_bytes: usize,
}

impl<V> Lru<V> {
    fn new(max_len: usize, max_bytes: usize) -> Self {
        Self {
            map: HashMap::new(),
            tick: 0,
            bytes: 0,
            max_len,
            max_bytes,
        }
    }

    fn get(&mut self, key: u64) -> Option<&V> {
        self.tick += 1;
        let tick = self.tick;
        self.map.get_mut(&key).map(|e| {
            e.2 = tick;
            &e.0
        })
    }

    /// Inserts and returns the evicted values, oldest first.
    fn insert(&mut self, key: u64, value: V, bytes: usize) -> Vec<V> {
        self.tick += 1;
        if let Some((_, old, _)) = self.map.insert(key, (value, bytes, self.tick)) {
            self.bytes -= old;
        }
        self.bytes += bytes;
        let mut evicted = Vec::new();
        while self.map.len() > 1 && (self.map.len() > self.max_len || self.bytes > self.max_bytes) {
            let Some(&oldest) = self
                .map
                .iter()
                .filter(|(k, _)| **k != key)
                .min_by_key(|(_, e)| e.2)
                .map(|(k, _)| k)
            else {
                break;
            };
            if let Some((v, b, _)) = self.map.remove(&oldest) {
                self.bytes -= b;
                evicted.push(v);
            }
        }
        evicted
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.map.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(w: u32, h: u32) -> Thumbnail {
        Thumbnail {
            url: format!("u{w}x{h}"),
            width: w,
            height: h,
        }
    }

    #[test]
    fn picks_smallest_that_covers() {
        let thumbs = [t(544, 544), t(60, 60), t(226, 226), t(120, 120)];
        assert_eq!(pick(&thumbs, 38).unwrap().width, 60);
        assert_eq!(pick(&thumbs, 60).unwrap().width, 60);
        assert_eq!(pick(&thumbs, 61).unwrap().width, 120);
        assert_eq!(pick(&thumbs, 148).unwrap().width, 226);
        // Nothing big enough: the largest.
        assert_eq!(pick(&thumbs, 900).unwrap().width, 544);
        assert!(pick(&[], 10).is_none());
    }

    #[test]
    fn picks_by_short_side_and_tolerates_unknown_sizes() {
        // A 480x360 video frame covers 300 px, a 320x180 one does not.
        let thumbs = [t(320, 180), t(480, 360)];
        assert_eq!(pick(&thumbs, 300).unwrap().width, 480);
        let unknown = [t(0, 0)];
        assert_eq!(pick(&unknown, 50).unwrap().url, "u0x0");
    }

    #[test]
    fn lru_evicts_least_recently_used() {
        let mut lru = Lru::new(3, usize::MAX);
        lru.insert(1, "a", 1);
        lru.insert(2, "b", 1);
        lru.insert(3, "c", 1);
        assert_eq!(lru.get(1), Some(&"a"));
        assert_eq!(lru.insert(4, "d", 1), vec!["b"]);
        assert_eq!(lru.insert(5, "e", 1), vec!["c"]);
        assert_eq!(lru.len(), 3);
        assert!(lru.get(2).is_none());
        assert_eq!(lru.get(1), Some(&"a"));
    }

    #[test]
    fn lru_respects_byte_budget() {
        let mut lru = Lru::new(100, 10);
        lru.insert(1, 1, 4);
        lru.insert(2, 2, 4);
        assert_eq!(lru.insert(3, 3, 4), vec![1]);
        // A single oversized entry is kept: it is the one being drawn.
        assert_eq!(lru.insert(4, 4, 50), vec![2, 3]);
        assert_eq!(lru.len(), 1);
    }

    #[test]
    fn cache_path_is_stable_hash_of_url() {
        let dir = Path::new("/c");
        // FNV-1a 64 test vectors.
        assert_eq!(fnv1a(FNV_OFFSET, b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a(FNV_OFFSET, b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(cache_path(dir, "a"), Path::new("/c/af63dc4c8601ec8c"));
        assert_ne!(
            cache_path(dir, "https://x/a.jpg"),
            cache_path(dir, "https://x/b.jpg")
        );
        assert_ne!(key("u", 38), key("u", 56));
    }

    fn job(key: u64) -> Job {
        Job {
            key,
            url: String::new(),
            px: 1,
        }
    }

    #[test]
    fn queue_is_newest_first_and_requests_once() {
        let mut q = Queue::new(8);
        assert!(q.request(job(1), 0));
        assert!(q.request(job(2), 0));
        assert!(!q.request(job(1), 0));
        assert_eq!(q.pop(0).unwrap().key, 2);
        // In flight: still not requested again until finished.
        assert!(!q.request(job(2), 0));
        q.finish(2);
        assert!(q.request(job(2), 0));
    }

    #[test]
    fn queue_drops_oldest_when_full() {
        let mut q = Queue::new(2);
        q.request(job(1), 0);
        q.request(job(2), 0);
        q.request(job(3), 0);
        assert_eq!(q.pop(0).unwrap().key, 3);
        assert_eq!(q.pop(0).unwrap().key, 2);
        assert!(q.pop(0).is_none());
        // The dropped one can be asked for again.
        assert!(q.request(job(1), 0));
    }

    #[test]
    fn queue_drops_rows_that_scrolled_away() {
        let mut q = Queue::new(8);
        q.request(job(1), 10); // scrolled away after pass 10
        q.request(job(2), 10);
        q.request(job(2), 20); // still on screen at pass 20
        assert_eq!(q.pop(20).unwrap().key, 2);
        assert!(q.pop(20).is_none());
        assert!(q.request(job(1), 21));
    }

    #[test]
    fn decodes_and_crops_to_square() {
        let mut png = Vec::new();
        image::RgbaImage::from_pixel(80, 40, image::Rgba([255, 0, 0, 255]))
            .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let img = decode(&png, 20).unwrap();
        assert_eq!(img.size, [20, 20]);
        // Never upscales past the source.
        assert_eq!(decode(&png, 100).unwrap().size, [40, 40]);
        assert!(decode(b"not an image", 20).is_none());
    }
}
