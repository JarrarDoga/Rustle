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
//!
//! `request_colors` runs a separate background pass that computes art
//! accent colors for many tracks without an egui `Context`. Both share the
//! disk cache, which is capped at `DISK_CAP` bytes and evicted least
//! recently used first (reads touch the file's mtime).

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, SystemTime};

use egui::{ColorImage, Context, Id, TextureHandle, TextureId, TextureOptions};
use model::{Thumbnail, Track, TrackId};

use crate::art_color;

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

/// One track for the color pass: its id and the URL of the thumbnail to
/// sample. Built by [`color_job`].
pub type ColorJob = (TrackId, String);

/// The color pass job for `track`: its smallest thumbnail. `None` when the
/// track has no art.
pub fn color_job(track: &Track) -> Option<ColorJob> {
    pick(&track.thumbnails, COLOR_PX).map(|t| (track.id.clone(), t.url.clone()))
}

/// Computes the art accent color for each job in a low-priority
/// background pass and hands results to `sink` in batches of up to
/// `COLOR_BATCH`, plus one at the end. The color sent is the accent
/// (`art_color::accent` of `art_color::dominant`), not the raw dominant
/// color: the spine and the player bar draw the accent.
///
/// Returns at once. At most `COLOR_WORKERS` thumbnails are fetched at a
/// time and no image is kept. Jobs carry only an id and a URL, not the
/// track, so a pass over a large library holds little. A call while a pass
/// runs adds its new tracks to that pass; the latest sink receives every
/// later batch. Tracks already done or queued this session are skipped, as
/// are tracks whose art cannot be decoded. After `MAX_FETCH_FAILURES`
/// failed fetches in a row (no network) the pass drops its queue; a later
/// call retries those tracks.
pub fn request_colors<F>(jobs: Vec<ColorJob>, sink: F)
where
    F: Fn(Vec<(model::TrackId, [u8; 3])>) + Send + Sync + 'static,
{
    static PASS: OnceLock<Arc<ColorPass>> = OnceLock::new();
    PASS.get_or_init(|| ColorPass::new(cache_dir()))
        .request(jobs, Arc::new(sink));
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
        let agent = agent();
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

/// The HTTP agent for thumbnail fetches. One per process, so the
/// thumbnail workers and the color pass share a TLS config and a
/// connection pool.
fn agent() -> ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT
        .get_or_init(|| {
            ureq::Agent::new_with_config(
                ureq::Agent::config_builder()
                    // Per-phase timeouts, no global one: with a global (or
                    // resolve) timeout, ureq resolves the host on a new
                    // thread for every request, pooled connection or not
                    // (1,500 threads in `perf-check.sh --large`). Without
                    // one it resolves inline, bounded by the system
                    // resolver's own timeout.
                    .timeout_connect(Some(Duration::from_secs(10)))
                    .timeout_send_request(Some(Duration::from_secs(10)))
                    .timeout_recv_response(Some(Duration::from_secs(20)))
                    .timeout_recv_body(Some(Duration::from_secs(20)))
                    // ureq's 128 KB defaults cost about 6 MB of peak RSS
                    // across the two workers; thumbnail responses need far
                    // less.
                    .input_buffer_size(16 << 10)
                    .output_buffer_size(4 << 10)
                    // Keep one idle connection per worker of either kind:
                    // they all hit the same CDN host.
                    .max_idle_connections_per_host(WORKERS + COLOR_WORKERS)
                    .build(),
            )
        })
        .clone()
}

/// Fetches (or reads from disk), decodes and downscales one thumbnail.
fn load(agent: &ureq::Agent, dir: Option<&Path>, url: &str, px: u32) -> Option<ColorImage> {
    let bytes = cached_or_fetch(agent, dir, url)?;
    decode(&bytes, px)
}

/// The bytes for `url`: from the disk cache if there, else fetched and
/// stored.
fn cached_or_fetch(agent: &ureq::Agent, dir: Option<&Path>, url: &str) -> Option<Vec<u8>> {
    let path = dir.map(|d| cache_path(d, url));
    if let Some(b) = path.as_deref().and_then(read_cached) {
        return Some(b);
    }
    let b = fetch(agent, url)?;
    if let (Some(dir), Some(path)) = (dir, &path) {
        store(dir, path, &b);
    }
    Some(b)
}

/// Reads a cached file and touches its mtime, which marks it recently used
/// for eviction.
fn read_cached(path: &Path) -> Option<Vec<u8>> {
    let Ok(mut f) = std::fs::File::options().read(true).write(true).open(path) else {
        // Read-only cache: still usable, just not reordered.
        return std::fs::read(path).ok();
    };
    let mut bytes = Vec::new();
    f.read_to_end(&mut bytes).ok()?;
    let _ = f.set_modified(SystemTime::now());
    Some(bytes)
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
    if std::fs::write(&tmp, bytes).is_err() {
        let _ = std::fs::remove_file(&tmp);
        return;
    }
    if std::fs::rename(&tmp, path).is_err() {
        let _ = std::fs::remove_file(&tmp);
        return;
    }
    note_stored(dir, bytes.len() as u64);
}

/// Disk cache size cap. Thumbnails are 2 to 50 KB, so this holds thousands.
const DISK_CAP: u64 = 100 << 20;
/// Eviction trims to this, so it runs once per ~10 MB of new files rather
/// than on every write.
const DISK_TARGET: u64 = 90 << 20;
/// Leftover temp files from a crash are removed once this old.
const STALE_TMP: Duration = Duration::from_secs(3600);

/// The cache directory and its size, counted on the first store of the
/// session and kept up to date after.
static DISK_USED: Mutex<Option<(PathBuf, u64)>> = Mutex::new(None);

/// Adds a stored file to the cache size and evicts when over `DISK_CAP`.
/// Runs on the worker that stored the file, never on the UI thread.
fn note_stored(dir: &Path, bytes: u64) {
    account(dir, bytes, DISK_CAP, DISK_TARGET);
}

fn account(dir: &Path, bytes: u64, cap: u64, target: u64) {
    let mut used = lock(&DISK_USED);
    let total = match used.as_mut() {
        Some((d, n)) if d == dir => {
            *n += bytes;
            *n
        }
        _ => {
            // The scan already counts the file just stored.
            let n = cache_entries(dir).map(|e| e.size).sum();
            *used = Some((dir.to_path_buf(), n));
            n
        }
    };
    if total > cap {
        let left = evict(dir, target);
        *used = Some((dir.to_path_buf(), left));
    }
}

/// One cached file: its name (the URL hash), last use and size.
struct Entry {
    name: u64,
    used: SystemTime,
    size: u64,
}

/// The cached files in `dir`. Removes stale temp files on the way and
/// skips anything else that is not a cache file.
fn cache_entries(dir: &Path) -> impl Iterator<Item = Entry> {
    let now = SystemTime::now();
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(move |e| {
            let meta = e.metadata().ok().filter(|m| m.is_file())?;
            let used = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            let file_name = e.file_name();
            let name = file_name.to_str()?;
            if name.len() == 16
                && let Ok(hash) = u64::from_str_radix(name, 16)
            {
                return Some(Entry {
                    name: hash,
                    used,
                    size: meta.len(),
                });
            }
            if name.contains(".tmp") && now.duration_since(used).is_ok_and(|age| age > STALE_TMP) {
                let _ = std::fs::remove_file(e.path());
            }
            None
        })
}

/// Deletes least recently used files until the cache holds at most
/// `target` bytes. Returns the bytes left.
fn evict(dir: &Path, target: u64) -> u64 {
    // About 32 bytes per file, so a full 100 MB cache costs ~300 KB here.
    let mut entries: Vec<Entry> = cache_entries(dir).collect();
    let mut total: u64 = entries.iter().map(|e| e.size).sum();
    entries.sort_unstable_by_key(|e| (e.used, e.name));
    for e in entries {
        if total <= target {
            break;
        }
        if std::fs::remove_file(dir.join(format!("{:016x}", e.name))).is_ok() {
            total -= e.size;
        }
    }
    total
}

/// Decodes and center-crops to a square of at most `px` pixels.
fn decode(bytes: &[u8], px: u32) -> Option<ColorImage> {
    decode_with(bytes, px, 32 << 20)
}

/// `decode` with a cap on the decoder's allocations.
fn decode_with(bytes: &[u8], px: u32, max_alloc: u64) -> Option<ColorImage> {
    let mut reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_DIM);
    limits.max_image_height = Some(MAX_DIM);
    limits.max_alloc = Some(max_alloc);
    reader.limits(limits);
    let img = reader.decode().ok()?;
    let side = px.min(img.width()).min(img.height()).max(1);
    let rgba = img
        .resize_to_fill(side, side, image::imageops::FilterType::Triangle)
        .into_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    Some(ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()))
}

/// Most thumbnails the color pass fetches at once. The pass may add at most
/// 3 MB of peak RSS. Measured with `perf-check.sh --large` (1,535 tracks
/// from a local server) once the app turned off transparent huge pages:
/// peak RSS 137.2 to 138.0 MiB with 1 worker, 137.6 to 137.8 with 2; all
/// colors in 21.4 s with 1, 16.0 to 17.3 s with 2. (Before that, each new
/// thread's heap was faulted in 2 MiB huge pages and 2 workers cost +3 MB.)
const COLOR_WORKERS: usize = 2;
/// Slot size handed to `pick`: the smallest thumbnail (60 px for songs).
const COLOR_PX: u32 = 32;
/// `art_color::dominant` samples a 16x16 grid, so decode to that.
const COLOR_DECODE_PX: u32 = 16;
/// Decoder allocation cap for the color pass. A 544 px JPEG fits; larger
/// art fails instead of costing several MB per worker.
const COLOR_MAX_ALLOC: u64 = 1 << 20;
/// Results per sink call.
const COLOR_BATCH: usize = 50;
/// Failed fetches in a row after which the pass gives up (no network).
const MAX_FETCH_FAILURES: usize = 8;

type ColorSink = Arc<dyn Fn(Vec<(TrackId, [u8; 3])>) + Send + Sync>;

/// The background color pass. Workers start on demand, up to
/// `COLOR_WORKERS`, and exit when the queue is empty, so an idle app keeps
/// no threads for it.
struct ColorPass {
    dir: Option<PathBuf>,
    state: Mutex<ColorState>,
}

struct ColorState {
    queue: VecDeque<ColorJob>,
    /// Queued, in flight, done or undecodable this session.
    seen: HashSet<TrackId>,
    done: Vec<(TrackId, [u8; 3])>,
    sink: Option<ColorSink>,
    active: usize,
    fetch_failures: usize,
}

/// What became of one track.
enum Colored {
    Color([u8; 3]),
    /// The thumbnail does not decode: not retried this session.
    Unusable,
    /// The fetch failed: retried by a later call.
    FetchFailed,
}

impl ColorPass {
    fn new(dir: Option<PathBuf>) -> Arc<Self> {
        Arc::new(Self {
            dir,
            state: Mutex::new(ColorState {
                queue: VecDeque::new(),
                seen: HashSet::new(),
                done: Vec::new(),
                sink: None,
                active: 0,
                fetch_failures: 0,
            }),
        })
    }

    fn request(self: &Arc<Self>, jobs: Vec<ColorJob>, sink: ColorSink) {
        let spawn = {
            let mut st = lock(&self.state);
            st.sink = Some(sink);
            for job in jobs {
                if st.seen.insert(job.0.clone()) {
                    st.queue.push_back(job);
                }
            }
            let spawn = COLOR_WORKERS.saturating_sub(st.active).min(st.queue.len());
            if spawn == 0 {
                return;
            }
            st.active += spawn;
            spawn
        };
        for i in 0..spawn {
            let pass = Arc::clone(self);
            let spawned = std::thread::Builder::new()
                .name(format!("rustle-colors-{i}"))
                .spawn(move || pass.work(&agent()));
            if let Err(e) = spawned {
                eprintln!("rustle: could not start color worker: {e}");
                let mut st = lock(&self.state);
                st.active -= 1;
                if st.active == 0 {
                    // Nobody left to run the queue: let a later call retry.
                    let ColorState { queue, seen, .. } = &mut *st;
                    for (id, _) in queue.drain(..) {
                        seen.remove(&id);
                    }
                }
            }
        }
    }

    fn work(&self, agent: &ureq::Agent) {
        lower_priority();
        loop {
            let (id, url) = {
                let mut st = lock(&self.state);
                match st.queue.pop_front() {
                    Some(t) => t,
                    None => {
                        // Exit under the same lock that saw the queue empty,
                        // so a request in between always finds `active`
                        // accurate and starts a worker if needed.
                        st.active -= 1;
                        let last = if st.active == 0 {
                            st.fetch_failures = 0;
                            Some((std::mem::take(&mut st.done), st.sink.clone()))
                        } else {
                            None
                        };
                        drop(st);
                        if let Some((batch, Some(sink))) = last
                            && !batch.is_empty()
                        {
                            sink(batch);
                        }
                        return;
                    }
                }
            };
            let result = color_of(agent, self.dir.as_deref(), &url);
            // Return the decode and download buffers to the OS (see `worker`).
            // SAFETY: mi_collect has no preconditions.
            unsafe { libmimalloc_sys::mi_collect(true) };
            let send = {
                let mut st = lock(&self.state);
                match result {
                    Colored::Color(c) => {
                        st.fetch_failures = 0;
                        st.done.push((id, c));
                    }
                    Colored::Unusable => {}
                    Colored::FetchFailed => {
                        st.seen.remove(&id);
                        st.fetch_failures += 1;
                        if st.fetch_failures >= MAX_FETCH_FAILURES {
                            let ColorState { queue, seen, .. } = &mut *st;
                            for (id, _) in queue.drain(..) {
                                seen.remove(&id);
                            }
                        }
                    }
                }
                if st.done.len() >= COLOR_BATCH {
                    Some((std::mem::take(&mut st.done), st.sink.clone()))
                } else {
                    None
                }
            };
            if let Some((batch, Some(sink))) = send {
                sink(batch);
            }
        }
    }
}

/// The accent color of the art at `url`.
fn color_of(agent: &ureq::Agent, dir: Option<&Path>, url: &str) -> Colored {
    let Some(bytes) = cached_or_fetch(agent, dir, url) else {
        return Colored::FetchFailed;
    };
    match decode_with(&bytes, COLOR_DECODE_PX, COLOR_MAX_ALLOC) {
        Some(img) => Colored::Color(art_color::accent(art_color::dominant(&img))),
        None => Colored::Unusable,
    }
}

/// Lowers the calling thread's CPU priority so the color pass yields to
/// the UI, audio and thumbnail threads. Linux keeps the nice value per
/// thread.
fn lower_priority() {
    #[cfg(target_os = "linux")]
    {
        unsafe extern "C" {
            fn nice(inc: std::ffi::c_int) -> std::ffi::c_int;
        }
        // SAFETY: nice has no preconditions; on failure the priority stays.
        unsafe { nice(10) };
    }
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

    // --- Disk cache ---

    /// A fresh directory under the system temp dir.
    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rustle-thumbs-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn file(dir: &Path, name: u64) -> PathBuf {
        dir.join(format!("{name:016x}"))
    }

    fn write_aged(path: &Path, len: usize, age_s: u64) {
        std::fs::write(path, vec![0u8; len]).unwrap();
        let when = SystemTime::now() - Duration::from_secs(age_s);
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(when)
            .unwrap();
    }

    fn names(dir: &Path) -> Vec<u64> {
        let mut v: Vec<u64> = cache_entries(dir).map(|e| e.name).collect();
        v.sort_unstable();
        v
    }

    #[test]
    fn evicts_least_recently_used_files() {
        let dir = temp_dir("evict");
        // Larger age = used longer ago.
        for (name, age) in [(1, 500), (2, 100), (3, 400), (4, 300), (5, 200)] {
            write_aged(&file(&dir, name), 10, age);
        }
        // Reading the oldest file makes it the most recently used.
        assert_eq!(read_cached(&file(&dir, 1)).unwrap().len(), 10);
        // Oldest first is now 3, 4, 5, 2, 1: trimming to 30 bytes drops 3 and 4.
        assert_eq!(evict(&dir, 30), 30);
        assert_eq!(names(&dir), [1, 2, 5]);
        assert_eq!(evict(&dir, 10), 10);
        assert_eq!(names(&dir), [1]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn eviction_ignores_other_files_and_clears_stale_temp_files() {
        let dir = temp_dir("evict-other");
        write_aged(&file(&dir, 7), 10, 10);
        write_aged(&dir.join("notes.txt"), 99, 9999);
        let stale = dir.join("00000000000000ab.tmpThreadId(3)");
        let fresh = dir.join("00000000000000ac.tmpThreadId(4)");
        write_aged(&stale, 5, 2 * 3600);
        write_aged(&fresh, 5, 10);
        assert_eq!(evict(&dir, 0), 0);
        assert!(dir.join("notes.txt").exists());
        assert!(!stale.exists());
        assert!(fresh.exists());
        assert!(names(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn storing_past_the_cap_evicts_down_to_the_target() {
        let dir = temp_dir("account");
        for name in 1..=4 {
            write_aged(&file(&dir, name), 10, 100 - name);
        }
        // The first store of a session counts the directory: 50 bytes.
        write_aged(&file(&dir, 5), 10, 0);
        account(&dir, 10, 45, 30);
        assert_eq!(names(&dir), [3, 4, 5]);
        // Later stores add to the count without a scan.
        write_aged(&file(&dir, 6), 10, 0);
        account(&dir, 10, 45, 30);
        assert_eq!(names(&dir), [3, 4, 5, 6]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // --- Color pass ---

    fn png(rgb: [u8; 3]) -> Vec<u8> {
        let mut out = Vec::new();
        image::RgbImage::from_pixel(60, 60, image::Rgb(rgb))
            .write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    /// A 60 px JPEG with some texture, about the size YouTube serves.
    fn jpeg(seed: u32) -> Vec<u8> {
        let img = image::RgbImage::from_fn(60, 60, |x, y| {
            let n = (x * 7 + y * 13 + seed * 31) % 64;
            image::Rgb([
                (seed * 40 % 192 + n) as u8,
                (seed * 90 % 192 + n / 2) as u8,
                (n * 3) as u8,
            ])
        });
        let mut out = Vec::new();
        img.write_to(&mut Cursor::new(&mut out), image::ImageFormat::Jpeg)
            .unwrap();
        out
    }

    fn track(id: &str, urls: &[String]) -> Track {
        Track {
            id: id.to_string(),
            title: id.to_string(),
            artists: Vec::new(),
            album: None,
            duration: None,
            thumbnails: urls
                .iter()
                .enumerate()
                .map(|(i, url)| Thumbnail {
                    url: url.clone(),
                    width: 60 << i,
                    height: 60 << i,
                })
                .collect(),
        }
    }

    /// The color job for a track with these thumbnails, smallest first.
    fn job_for(id: &str, urls: &[String]) -> ColorJob {
        color_job(&track(id, urls)).expect("has art")
    }

    type Batches = std::sync::mpsc::Receiver<Vec<(TrackId, [u8; 3])>>;

    fn sink() -> (ColorSink, Batches) {
        let (tx, rx) = std::sync::mpsc::channel();
        let sink: ColorSink = Arc::new(move |batch| {
            let _ = tx.send(batch);
        });
        (sink, rx)
    }

    /// Receives batches until `want` results came in or 20 s passed.
    fn collect(rx: &Batches, want: usize) -> (Vec<usize>, HashMap<TrackId, [u8; 3]>) {
        let mut sizes = Vec::new();
        let mut all = HashMap::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while all.len() < want {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            let Ok(batch) = rx.recv_timeout(left) else {
                break;
            };
            sizes.push(batch.len());
            for (id, c) in batch {
                assert!(all.insert(id.clone(), c).is_none(), "{id} sent twice");
            }
        }
        (sizes, all)
    }

    fn wait_idle(pass: &ColorPass) {
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        loop {
            {
                let st = lock(&pass.state);
                if st.active == 0 && st.queue.is_empty() {
                    return;
                }
            }
            assert!(std::time::Instant::now() < deadline, "pass never finished");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Port 9 (discard) is closed on a normal machine, so an uncached URL
    /// fails at once instead of reaching anything.
    fn dead_url(i: usize) -> String {
        format!("http://127.0.0.1:9/{i}.png")
    }

    #[test]
    fn color_pass_reads_the_disk_cache_and_sends_batches() {
        let dir = temp_dir("colors-cached");
        let hues = [[200, 30, 40], [30, 60, 200], [40, 160, 60]];
        let mut tracks = Vec::new();
        for i in 0..120 {
            let url = dead_url(i);
            std::fs::write(cache_path(&dir, &url), png(hues[i % 3])).unwrap();
            // The larger thumbnail is not cached: the job must pick the small one.
            tracks.push(job_for(&format!("t{i}"), &[url, dead_url(1000 + i)]));
        }
        let broken = dead_url(500);
        std::fs::write(cache_path(&dir, &broken), b"not an image").unwrap();
        tracks.push(job_for("broken", &[broken]));
        assert!(color_job(&track("no-art", &[])).is_none());

        let pass = ColorPass::new(Some(dir.clone()));
        let (s, rx) = sink();
        pass.request(tracks.clone(), s);
        let (sizes, all) = collect(&rx, 120);
        wait_idle(&pass);
        assert_eq!(all.len(), 120);
        assert!(sizes.iter().all(|&n| n <= COLOR_BATCH), "{sizes:?}");
        assert!(sizes.len() >= 3, "{sizes:?}");
        for i in 0..120 {
            let want = art_color::accent(hues[i % 3]);
            let got = all[&format!("t{i}")];
            assert!(
                (0..3).all(|k| got[k].abs_diff(want[k]) <= 1),
                "{got:?} vs {want:?}"
            );
        }
        assert!(!all.contains_key("broken"));

        // Asking again sends nothing new: done and unusable tracks are skipped.
        let (s, rx) = sink();
        pass.request(tracks, s);
        wait_idle(&pass);
        assert!(rx.recv_timeout(Duration::from_millis(100)).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Serves `bodies[n]` for `GET /n`. Records the most connections open at
    /// once. One thread per connection when `threaded`, else one at a time.
    fn serve(bodies: Vec<Vec<u8>>, threaded: bool) -> (u16, Arc<Mutex<(usize, usize)>>) {
        use std::io::Write;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let bodies = Arc::new(bodies);
        let open = Arc::new(Mutex::new((0usize, 0usize)));
        let counter = Arc::clone(&open);
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(mut conn) = conn else { continue };
                let bodies = Arc::clone(&bodies);
                let open = Arc::clone(&counter);
                let mut handle = move || {
                    {
                        let mut o = lock(&open);
                        o.0 += 1;
                        o.1 = o.1.max(o.0);
                    }
                    let mut req = Vec::new();
                    let mut buf = [0u8; 1024];
                    while !req.windows(4).any(|w| w == b"\r\n\r\n") {
                        match conn.read(&mut buf) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => req.extend_from_slice(&buf[..n]),
                        }
                    }
                    let path = String::from_utf8_lossy(&req)
                        .split_whitespace()
                        .nth(1)
                        .unwrap_or("/")
                        .to_string();
                    let body = path
                        .trim_start_matches('/')
                        .parse::<usize>()
                        .ok()
                        .and_then(|n| bodies.get(n));
                    if threaded {
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    let _ = match body {
                        Some(b) => conn
                            .write_all(
                                format!(
                                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                    b.len()
                                )
                                .as_bytes(),
                            )
                            .and_then(|()| conn.write_all(b)),
                        None => conn.write_all(
                            b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                        ),
                    };
                    lock(&open).0 -= 1;
                };
                if threaded {
                    std::thread::spawn(handle);
                } else {
                    handle();
                }
            }
        });
        (port, open)
    }

    #[test]
    fn color_pass_fetches_within_the_worker_limit_and_caches() {
        let dir = temp_dir("colors-fetch");
        let (port, open) = serve((0..24).map(jpeg).collect(), true);
        let tracks: Vec<_> = (0..24)
            .map(|i| job_for(&format!("f{i}"), &[format!("http://127.0.0.1:{port}/{i}")]))
            .collect();
        let pass = ColorPass::new(Some(dir.clone()));
        let (s, rx) = sink();
        pass.request(tracks, s);
        let (_, all) = collect(&rx, 24);
        wait_idle(&pass);
        assert_eq!(all.len(), 24);
        let most = lock(&open).1;
        assert!((1..=COLOR_WORKERS).contains(&most), "{most} at once");
        assert_eq!(cache_entries(&dir).count(), 24);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn color_pass_merges_calls_and_gives_up_without_network() {
        let dir = temp_dir("colors-offline");
        let pass = ColorPass::new(Some(dir.clone()));
        let offline: Vec<_> = (0..30)
            .map(|i| job_for(&format!("o{i}"), &[dead_url(i)]))
            .collect();
        let (s, rx) = sink();
        pass.request(offline.clone(), s);
        // A second call with overlapping tracks while the first runs.
        let (s, rx2) = sink();
        pass.request(offline[20..].to_vec(), s);
        wait_idle(&pass);
        assert!(rx.try_recv().is_err() && rx2.try_recv().is_err());
        {
            let st = lock(&pass.state);
            // Nothing is remembered, so a later call retries every track.
            assert!(st.seen.is_empty(), "{:?}", st.seen);
            assert_eq!(st.fetch_failures, 0);
        }

        // Back online (cached now): the same tracks get colors, each once.
        for i in 0..30 {
            std::fs::write(cache_path(&dir, &dead_url(i)), png([200, 30, 40])).unwrap();
        }
        let (s, rx) = sink();
        pass.request(offline[..20].to_vec(), s);
        let (s, rx2) = sink();
        pass.request(offline.clone(), s);
        // Batches before the second call go to the first sink, the rest to
        // the second.
        let mut all = HashMap::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while all.len() < 30 && std::time::Instant::now() < deadline {
            for rx in [&rx, &rx2] {
                while let Ok(batch) = rx.try_recv() {
                    for (id, c) in batch {
                        assert!(all.insert(id.clone(), c).is_none(), "{id} sent twice");
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        wait_idle(&pass);
        assert_eq!(all.len(), 30);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn proc_kb(field: &str) -> u64 {
        std::fs::read_to_string("/proc/self/status")
            .unwrap()
            .lines()
            .find_map(|l| {
                l.strip_prefix(field)?
                    .trim()
                    .strip_suffix("kB")?
                    .trim()
                    .parse()
                    .ok()
            })
            .unwrap()
    }

    /// Peak RSS the color pass adds over 200 tracks, fetched from a local
    /// keep-alive server and then from the disk cache. The server is a
    /// Python child process, so its memory is not counted. Run alone, in
    /// release:
    /// `cargo test --release -p rustle color_pass_rss -- --ignored --test-threads=1 --nocapture`
    #[test]
    #[ignore = "measurement; run alone in release, needs python3"]
    fn color_pass_rss() {
        use std::io::BufRead;
        const N: usize = 200;
        const SERVER: &str = "import sys, http.server as h\n\
            class H(h.SimpleHTTPRequestHandler):\n\
            \x20   protocol_version = 'HTTP/1.1'\n\
            \x20   def __init__(s, *a, **k): super().__init__(*a, directory=sys.argv[1], **k)\n\
            \x20   def log_message(s, *a): pass\n\
            srv = h.ThreadingHTTPServer(('127.0.0.1', 0), H)\n\
            print(srv.server_address[1], flush=True)\n\
            srv.serve_forever()\n";
        let dir = temp_dir("colors-rss");
        let served = temp_dir("colors-rss-served");
        let mut bytes = 0;
        for i in 0..N {
            let b = jpeg(i as u32);
            bytes += b.len();
            std::fs::write(served.join(i.to_string()), b).unwrap();
        }
        let mut server = std::process::Command::new("python3")
            .args(["-I", "-c", SERVER])
            .arg(&served)
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("python3");
        let mut line = String::new();
        std::io::BufReader::new(server.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        let port: u16 = line.trim().parse().unwrap();
        let tracks: Vec<_> = (0..N)
            .map(|i| job_for(&format!("r{i}"), &[format!("http://127.0.0.1:{port}/{i}")]))
            .collect();
        // The app's thumbnail workers have already used the shared agent.
        let warm = format!("http://127.0.0.1:{port}/0");
        std::thread::spawn(move || fetch(&agent(), &warm))
            .join()
            .unwrap();
        for (label, tracks) in [("fetch", tracks.clone()), ("disk cache", tracks)] {
            // Reset VmHWM to the current RSS (Linux 4.0+).
            std::fs::write("/proc/self/clear_refs", "5").unwrap();
            let rss0 = proc_kb("VmRSS:");
            let pass = ColorPass::new(Some(dir.clone()));
            let (s, rx) = sink();
            let t0 = std::time::Instant::now();
            pass.request(tracks, s);
            let (_, all) = collect(&rx, N);
            wait_idle(&pass);
            assert_eq!(all.len(), N);
            let (rss1, hwm1) = (proc_kb("VmRSS:"), proc_kb("VmHWM:"));
            eprintln!(
                "color pass ({label}, {N} tracks, {} KB of JPEG, {:?}): peak RSS {rss0} -> {hwm1} kB (+{} kB), RSS after {rss1} kB (+{} kB)",
                bytes / 1024,
                t0.elapsed(),
                hwm1.saturating_sub(rss0),
                rss1 as i64 - rss0 as i64
            );
        }
        let _ = server.kill();
        let _ = server.wait();
        let _ = std::fs::remove_dir_all(&served);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
