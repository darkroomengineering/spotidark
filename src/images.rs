//! Album art: fetched once, kept on disk, decoded by egui on demand.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use egui::load::{
    Bytes, BytesLoadResult, BytesLoader, BytesPoll, ImageLoadResult, ImageLoader, ImagePoll,
    LoadError, SizeHint,
};
use image::ImageDecoder;
use sha1::{Digest, Sha1};
use tokio::sync::Semaphore;

use crate::http::Http;

/// Maximum artwork bytes held in memory.
///
/// Time-based eviction does not work here: after creating a texture, egui no
/// longer requests its source bytes. Visible images were therefore evicted and
/// reloaded every two and a half minutes (#129).
///
/// Size-based eviction keeps visible images stable.
const HELD_BYTES: usize = 64 * 1024 * 1024;
const MAX_ART_BYTES: usize = 8 * 1024 * 1024;
const FETCH_JOBS: usize = 4;
const DECODE_JOBS: usize = 2;
const PENDING_ART: usize = 32;
static REQUEST_ID: AtomicU64 = AtomicU64::new(1);

fn request_id() -> u64 {
    REQUEST_ID.fetch_add(1, Ordering::Relaxed)
}

/// Decoded ColorImage plus the GPU texture, both RGBA.
fn decoded_and_texture_bytes(width: usize, height: usize) -> usize {
    2 * width.saturating_mul(height).saturating_mul(4)
}

/// What this loader answers for. egui offers it every URI, and artwork that
/// is not fetched over the network belongs to another loader.
fn is_http(uri: &str) -> bool {
    uri.starts_with("https://") || uri.starts_with("http://")
}

struct Entry {
    request: Option<u64>,
    bytes: Option<Arc<[u8]>>,
    error: Option<String>,
    decoded: Option<Result<Arc<egui::ColorImage>, String>>,
    decoding: Option<u64>,
    last_used: Instant,
    /// Decoded image and GPU texture once painted, excluding compressed bytes.
    raster_bytes: usize,
}

impl Default for Entry {
    fn default() -> Self {
        Self {
            request: None,
            bytes: None,
            error: None,
            decoded: None,
            decoding: None,
            last_used: Instant::now(),
            raster_bytes: 0,
        }
    }
}

impl Entry {
    fn retained_bytes(&self) -> usize {
        self.bytes.as_ref().map_or(0, |bytes| bytes.len()) + self.raster_bytes
    }
}

struct Inner {
    entries: Mutex<HashMap<String, Entry>>,
    http: Http,
    runtime: tokio::runtime::Handle,
    cache_dir: PathBuf,
    fetch_jobs: Arc<Semaphore>,
    decode_jobs: Arc<Semaphore>,
    accent_jobs: Arc<Semaphore>,
}

#[derive(Clone)]
pub struct ArtLoader {
    inner: Arc<Inner>,
}

impl ArtLoader {
    pub fn new(http: impl Into<Http>, runtime: tokio::runtime::Handle, cache_dir: PathBuf) -> Self {
        let _ = std::fs::create_dir_all(&cache_dir);
        Self {
            inner: Arc::new(Inner {
                entries: Mutex::new(HashMap::new()),
                http: http.into(),
                runtime,
                cache_dir,
                fetch_jobs: Arc::new(Semaphore::new(FETCH_JOBS)),
                decode_jobs: Arc::new(Semaphore::new(DECODE_JOBS)),
                accent_jobs: Arc::new(Semaphore::new(2)),
            }),
        }
    }

    /// Bytes for `url`, from memory, disk, or the network.
    pub async fn fetch(&self, url: &str) -> Result<Arc<[u8]>, String> {
        self.inner.fetch(url).await
    }

    /// HTTP artwork uses the same cache and decode budget as accent/backdrop work.
    pub fn image_loader(&self) -> impl ImageLoader + Send + Sync + 'static {
        ArtImageLoader(self.clone())
    }

    pub async fn accent(&self, url: &str) -> Option<[u8; 3]> {
        // Slow accent downloads cannot occupy the display decoder slots. At
        // most two downloaded accent payloads wait for a decoder.
        let accent = self.inner.accent_jobs.clone().acquire_owned().await.ok()?;
        let bytes = self.fetch(url).await.ok()?;
        let decode = self.inner.decode_jobs.clone().acquire_owned().await.ok()?;
        self.inner
            .runtime
            .spawn_blocking(move || {
                let _permits = (accent, decode);
                accent_color(&bytes)
            })
            .await
            .ok()
            .flatten()
    }

    /// Marks artwork as visible so size-based eviction keeps it stable.
    pub fn touch(&self, url: &str) {
        if let Some(entry) = self
            .inner
            .entries
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get_mut(url)
        {
            entry.last_used = Instant::now();
        }
    }

    /// Evicts failed entries and the oldest artwork above the memory limit.
    pub fn evict(&self, ctx: &egui::Context) {
        let letting_go: Vec<String> = {
            let mut entries = self.inner.entries.lock().unwrap_or_else(|p| p.into_inner());
            let mut failed: Vec<String> = Vec::new();
            let mut held: Vec<(String, Instant, usize)> = Vec::new();
            for (url, entry) in entries.iter_mut() {
                if entry.request.is_some() || entry.decoding.is_some() {
                    continue;
                }
                // A failed later byte reload (for lyrics) must remain
                // retryable without making the already visible cover blink.
                if entry.decoded.as_ref().is_some_and(Result::is_ok) {
                    entry.error = None;
                }
                if entry.decoded.as_ref().is_some_and(Result::is_err)
                    || (entry.error.is_some() && entry.decoded.is_none())
                {
                    failed.push(url.clone());
                } else {
                    held.push((url.clone(), entry.last_used, entry.retained_bytes()));
                }
            }
            failed.extend(over_budget(held, HELD_BYTES));
            failed
        };
        for url in letting_go {
            ctx.forget_image(&url);
            self.forget(&url);
        }
    }

    /// The disk-cache file holding `url`'s artwork, once it has been fetched.
    ///
    /// The cache is written atomically (a `.part` file, then a rename), so a
    /// file that is here at all holds a complete, successful response. The
    /// desktop media controls hand this path to the platform instead of the
    /// remote URL: macOS loads cover art itself, synchronously, inside a
    /// callback that cannot report a failure.
    pub fn cached_file(&self, url: &str) -> Option<PathBuf> {
        let path = self.inner.cache_path(url);
        std::fs::metadata(&path)
            .is_ok_and(|meta| meta.is_file() && (1..=MAX_ART_BYTES as u64).contains(&meta.len()))
            .then_some(path)
    }

    /// Starts the download for `url` while nothing is drawing it, so the
    /// media controls have a file to hand the platform, and answers whether
    /// this call is what started it.
    ///
    /// Artwork already held, already on its way, or addressed by a scheme
    /// this loader does not answer for is left alone.
    pub fn prefetch(&self, ctx: &egui::Context, url: &str) -> bool {
        if !is_http(url) {
            return false;
        }
        let mut entries = self.inner.entries.lock().unwrap_or_else(|p| p.into_inner());
        if entries.contains_key(url)
            || entries
                .values()
                .filter(|entry| entry.request.is_some())
                .count()
                >= PENDING_ART
        {
            return false;
        }
        let request = request_id();
        entries.insert(
            url.to_string(),
            Entry {
                request: Some(request),
                ..Entry::default()
            },
        );
        drop(entries);
        self.inner.start(ctx, url.to_string(), request);
        true
    }

    /// Drops held JPEG bytes once egui has made a texture. The disk cache
    /// remains for later reloads.
    pub fn release_bytes(&self, url: &str) {
        self.inner.drop_bytes(url);
    }

    /// Record decoded image + texture size after egui has uploaded the cover.
    pub fn note_decoded(&self, url: &str, width: usize, height: usize) {
        if let Some(entry) = self
            .inner
            .entries
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get_mut(url)
        {
            entry.raster_bytes = decoded_and_texture_bytes(width, height);
        }
    }

    pub fn clear_disk_cache(&self) -> std::io::Result<u64> {
        let mut removed = 0;
        for entry in std::fs::read_dir(&self.inner.cache_dir)? {
            let entry = entry?;
            if entry.file_type()?.is_file() {
                removed += entry.metadata().map(|m| m.len()).unwrap_or(0);
                let _ = std::fs::remove_file(entry.path());
            }
        }
        Ok(removed)
    }
}

/// Which artwork to let go of so that what is kept fits `budget`,
/// oldest first.
///
/// "Oldest" is when egui last needed the bytes, which for a picture it
/// has already made a texture of is when it first loaded. That makes
/// this a rough order rather than a true reading of what is on screen,
/// which is why the budget is generous: being roughly right about which
/// to drop only matters once there is far more artwork than any window
/// is showing.
fn over_budget(mut held: Vec<(String, Instant, usize)>, budget: usize) -> Vec<String> {
    let mut total: usize = held.iter().map(|(_, _, bytes)| bytes).sum();
    if total <= budget {
        return Vec::new();
    }
    held.sort_by_key(|(_, last_used, _)| *last_used);
    let mut letting_go = Vec::new();
    for (url, _, bytes) in held {
        if total <= budget {
            break;
        }
        total = total.saturating_sub(bytes);
        letting_go.push(url);
    }
    letting_go
}

impl Inner {
    fn cache_path(&self, url: &str) -> PathBuf {
        let digest = Sha1::digest(url.as_bytes());
        let mut name = String::with_capacity(40);
        for byte in digest {
            use std::fmt::Write;
            let _ = write!(name, "{byte:02x}");
        }
        self.cache_dir.join(name)
    }

    async fn fetch(self: &Arc<Self>, url: &str) -> Result<Arc<[u8]>, String> {
        if let Some(bytes) = self.held_bytes(url) {
            return Ok(bytes);
        }
        let permit = self
            .fetch_jobs
            .clone()
            .acquire_owned()
            .await
            .map_err(|error| error.to_string())?;
        if let Some(bytes) = self.held_bytes(url) {
            return Ok(bytes);
        }
        let path = self.cache_path(url);
        // A blocking read owns its permit even when its waiting future is
        // cancelled. The same applies to writes and all decoding jobs below.
        let (cached, permit) = tokio::task::spawn_blocking({
            let path = path.clone();
            move || (read_cached_art(&path), permit)
        })
        .await
        .map_err(|error| error.to_string())?;
        let bytes: Arc<[u8]> = match cached {
            Some(bytes) => Arc::from(bytes),
            _ => {
                let mut response = self
                    .http
                    .client()?
                    .get(url)
                    .send()
                    .await
                    .map_err(|error| error.to_string())?;
                if !response.status().is_success() {
                    return Err(format!("artwork request failed: {}", response.status()));
                }
                if response
                    .content_length()
                    .is_some_and(|length| length > MAX_ART_BYTES as u64)
                {
                    return Err("artwork is too large".to_string());
                }
                let mut bytes = Vec::new();
                while let Some(chunk) = response.chunk().await.map_err(|error| error.to_string())? {
                    if chunk.len() > MAX_ART_BYTES.saturating_sub(bytes.len()) {
                        return Err("artwork is too large".to_string());
                    }
                    bytes.extend_from_slice(&chunk);
                }
                // The loader and file worker share one immutable payload.
                let bytes: Arc<[u8]> = Arc::from(bytes);
                let write_path = path.clone();
                let payload = Arc::clone(&bytes);
                self.runtime.spawn_blocking(move || {
                    let _permit = permit;
                    write_cached_art(&write_path, &payload);
                });
                bytes
            }
        };
        Ok(bytes)
    }

    fn held_bytes(&self, url: &str) -> Option<Arc<[u8]>> {
        self.entries
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(url)
            .and_then(|entry| entry.bytes.clone())
    }

    fn start(self: &Arc<Self>, ctx: &egui::Context, url: String, request: u64) {
        let loader = Arc::clone(self);
        let ctx = ctx.clone();
        self.runtime.spawn(async move {
            let result = loader.fetch(&url).await;
            loader.complete(&url, request, result);
            ctx.request_repaint();
        });
    }

    fn complete(&self, url: &str, request: u64, result: Result<Arc<[u8]>, String>) {
        let mut entries = self.entries.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(entry) = entries
            .get_mut(url)
            .filter(|entry| entry.request == Some(request))
        {
            entry.request = None;
            match result {
                Ok(bytes) => {
                    entry.bytes = Some(bytes);
                    entry.error = None;
                }
                Err(error) => entry.error = Some(error),
            }
        }
    }

    fn drop_bytes(&self, url: &str) {
        if let Some(entry) = self
            .entries
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get_mut(url)
        {
            entry.bytes = None;
        }
    }
}

fn read_cached_art(path: &std::path::Path) -> Option<Vec<u8>> {
    let file = std::fs::File::open(path).ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.is_file() || !(1..=MAX_ART_BYTES as u64).contains(&metadata.len()) {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(MAX_ART_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    (!bytes.is_empty() && bytes.len() <= MAX_ART_BYTES).then_some(bytes)
}

fn write_cached_art(path: &std::path::Path, bytes: &[u8]) {
    // Direct callers can request the same URL concurrently. Each writer owns
    // its temporary file, including when two loaders share a cache directory.
    let temporary = path.with_extension(format!("part-{}-{}", std::process::id(), request_id()));
    let Ok(mut file) = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
    else {
        return;
    };
    let written = file.write_all(bytes);
    drop(file);
    if written.is_err() || crate::util::replace_file(&temporary, path).is_err() {
        let _ = std::fs::remove_file(temporary);
    }
}

impl BytesLoader for ArtLoader {
    fn id(&self) -> &'static str {
        "fastpotify::ArtLoader"
    }

    fn load(&self, ctx: &egui::Context, uri: &str) -> BytesLoadResult {
        if !is_http(uri) {
            return Err(LoadError::NotSupported);
        }
        let mut entries = self.inner.entries.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(entry) = entries.get_mut(uri) {
            entry.last_used = Instant::now();
            if let Some(bytes) = &entry.bytes {
                return Ok(BytesPoll::Ready {
                    size: None,
                    bytes: Bytes::Shared(Arc::clone(bytes)),
                    mime: None,
                });
            }
            if let Some(error) = &entry.error {
                return Err(LoadError::Loading(error.clone()));
            }
            if entry.request.is_some() {
                return Ok(BytesPoll::Pending { size: None });
            }
        }
        if entries
            .values()
            .filter(|entry| entry.request.is_some())
            .count()
            >= PENDING_ART
        {
            drop(entries);
            ctx.request_repaint_after(Duration::from_millis(50));
            return Ok(BytesPoll::Pending { size: None });
        }
        let request = request_id();
        // A later lyrics backdrop may need compressed bytes again. Preserve
        // the decoded image and texture accounting throughout that reload.
        entries.entry(uri.to_string()).or_default().request = Some(request);
        drop(entries);
        self.inner.start(ctx, uri.to_string(), request);
        Ok(BytesPoll::Pending { size: None })
    }

    fn forget(&self, uri: &str) {
        self.inner
            .entries
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(uri);
    }

    fn forget_all(&self) {
        self.inner
            .entries
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clear();
    }

    fn byte_size(&self) -> usize {
        self.inner
            .entries
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .values()
            .map(|entry| entry.bytes.as_ref().map_or(0, |bytes| bytes.len()))
            .sum()
    }

    fn has_pending(&self) -> bool {
        self.inner
            .entries
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .values()
            .any(|entry| entry.request.is_some())
    }
}

/// Installed after egui's default loaders, so HTTP art never reaches their
/// unbounded per-image thread spawning. Local/SVG assets keep their loaders.
struct ArtImageLoader(ArtLoader);

impl ImageLoader for ArtImageLoader {
    fn id(&self) -> &str {
        "fastpotify::ArtImageLoader"
    }

    fn load(&self, ctx: &egui::Context, uri: &str, _: SizeHint) -> ImageLoadResult {
        if !is_http(uri) {
            return Err(LoadError::NotSupported);
        }
        {
            let mut entries = self
                .0
                .inner
                .entries
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            if let Some(entry) = entries.get_mut(uri) {
                entry.last_used = Instant::now();
                if let Some(decoded) = &entry.decoded {
                    return decoded
                        .clone()
                        .map(|image| ImagePoll::Ready { image })
                        .map_err(LoadError::Loading);
                }
                if entry.decoding.is_some() {
                    return Ok(ImagePoll::Pending { size: None });
                }
            }
        }
        let bytes = match self.0.load(ctx, uri)? {
            BytesPoll::Pending { size } => return Ok(ImagePoll::Pending { size }),
            BytesPoll::Ready { bytes, .. } => bytes,
        };
        let Ok(permit) = self.0.inner.decode_jobs.clone().try_acquire_owned() else {
            ctx.request_repaint_after(Duration::from_millis(50));
            return Ok(ImagePoll::Pending { size: None });
        };
        let request = request_id();
        {
            let mut entries = self
                .0
                .inner
                .entries
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            let Some(entry) = entries.get_mut(uri) else {
                return Ok(ImagePoll::Pending { size: None });
            };
            if entry.decoding.is_some() || entry.decoded.is_some() {
                return Ok(ImagePoll::Pending { size: None });
            }
            entry.decoding = Some(request);
        }
        let loader = self.0.clone();
        let ctx = ctx.clone();
        let uri = uri.to_string();
        self.0.inner.runtime.spawn_blocking(move || {
            let _permit = permit;
            let decoded = decode_art(&bytes).map(|image| {
                let rgba = image.into_rgba8();
                Arc::new(egui::ColorImage::from_rgba_unmultiplied(
                    [rgba.width() as usize, rgba.height() as usize],
                    rgba.as_raw(),
                ))
            });
            {
                let mut entries = loader
                    .inner
                    .entries
                    .lock()
                    .unwrap_or_else(|p| p.into_inner());
                if let Some(entry) = entries
                    .get_mut(&uri)
                    .filter(|entry| entry.decoding == Some(request))
                {
                    entry.decoding = None;
                    if let Ok(image) = &decoded {
                        entry.raster_bytes =
                            decoded_and_texture_bytes(image.size[0], image.size[1]);
                    }
                    entry.decoded = Some(decoded);
                }
            }
            ctx.request_repaint();
        });
        Ok(ImagePoll::Pending { size: None })
    }

    fn forget(&self, uri: &str) {
        self.0.forget(uri);
    }
    fn forget_all(&self) {
        self.0.forget_all();
    }
    fn byte_size(&self) -> usize {
        self.0
            .inner
            .entries
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .values()
            .filter_map(|entry| entry.decoded.as_ref()?.as_ref().ok())
            .map(|image| image.pixels.len() * std::mem::size_of::<egui::Color32>())
            .sum()
    }
    fn has_pending(&self) -> bool {
        self.0
            .inner
            .entries
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .values()
            .any(|entry| entry.request.is_some() || entry.decoding.is_some())
    }
}

/// A colour that represents an album cover, suitable for tinting a dark or
/// light surface: the most common saturated hue, with its lightness pulled
/// into a range that still reads as a background.
pub fn accent_color(bytes: &[u8]) -> Option<[u8; 3]> {
    let decoded = decode_art(bytes).ok()?;
    let small = decoded.thumbnail(48, 48).to_rgb8();
    let mut buckets: HashMap<(u8, u8, u8), (u64, [u64; 3])> = HashMap::new();
    for pixel in small.pixels() {
        let [r, g, b] = pixel.0;
        let (max, min) = (r.max(g).max(b) as f32, r.min(g).min(b) as f32);
        let saturation = if max == 0.0 { 0.0 } else { (max - min) / max };
        let lightness = (max + min) / 510.0;
        // Weight toward vivid mid-tones so black borders and white text lose.
        let weight = (1.0 + saturation * 6.0) * (1.0 - (lightness - 0.5).abs() * 1.4).max(0.05);
        let weight = (weight * 100.0) as u64;
        let key = (r >> 4, g >> 4, b >> 4);
        let bucket = buckets.entry(key).or_insert((0, [0, 0, 0]));
        bucket.0 += weight;
        bucket.1[0] += r as u64 * weight;
        bucket.1[1] += g as u64 * weight;
        bucket.1[2] += b as u64 * weight;
    }
    let (_, (weight, sum)) = buckets.into_iter().max_by_key(|(_, (weight, _))| *weight)?;
    if weight == 0 {
        return None;
    }
    Some([
        (sum[0] / weight) as u8,
        (sum[1] / weight) as u8,
        (sum[2] / weight) as u8,
    ])
}

#[derive(Default)]
pub struct LyricsBackdrop {
    uri: Option<String>,
    requested: bool,
    pending: Option<std::sync::mpsc::Receiver<Option<egui::ColorImage>>>,
    texture: Option<egui::TextureHandle>,
}

impl LyricsBackdrop {
    pub fn texture(
        &mut self,
        ctx: &egui::Context,
        loader: &ArtLoader,
        uri: Option<&str>,
    ) -> Option<&egui::TextureHandle> {
        if self.uri.as_deref() != uri {
            *self = Self {
                uri: uri.map(str::to_owned),
                ..Default::default()
            };
        }
        let uri = uri?;
        if !self.requested {
            match ctx.try_load_bytes(uri) {
                Ok(BytesPoll::Ready { bytes, .. }) => {
                    let Ok(permit) = loader.inner.decode_jobs.clone().try_acquire_owned() else {
                        ctx.request_repaint_after(Duration::from_millis(50));
                        return None;
                    };
                    self.requested = true;
                    let (tx, rx) = std::sync::mpsc::channel();
                    self.pending = Some(rx);
                    let ctx = ctx.clone();
                    loader.inner.runtime.spawn_blocking(move || {
                        let _permit = permit;
                        let _ = tx.send(lyrics_background(&bytes));
                        ctx.request_repaint();
                    });
                }
                Err(error) => self.requested = terminal_lyrics_backdrop_error(&error),
                Ok(BytesPoll::Pending { .. }) => {}
            }
        }
        if let Some(receiver) = &self.pending {
            match receiver.try_recv() {
                Ok(image) => {
                    self.texture = image.map(|image| {
                        ctx.load_texture("lyrics-backdrop", image, egui::TextureOptions::LINEAR)
                    });
                    self.pending = None;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => self.pending = None,
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
        }
        self.texture.as_ref()
    }
}

fn terminal_lyrics_backdrop_error(error: &LoadError) -> bool {
    matches!(error, LoadError::NotSupported)
}

fn decode_art(bytes: &[u8]) -> Result<image::DynamicImage, String> {
    if bytes.len() > MAX_ART_BYTES {
        return Err("artwork is too large".into());
    }
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|error| error.to_string())?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits.clone());
    let mut decoder = reader.into_decoder().map_err(|error| error.to_string())?;
    // As in playlist-cover decoding, account for the output allocation when
    // using from_decoder to preserve the photograph's EXIF orientation.
    limits
        .reserve(decoder.total_bytes())
        .and_then(|()| decoder.set_limits(limits))
        .map_err(|error| error.to_string())?;
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut image =
        image::DynamicImage::from_decoder(decoder).map_err(|error| error.to_string())?;
    image.apply_orientation(orientation);
    Ok(image)
}

fn lyrics_background(bytes: &[u8]) -> Option<egui::ColorImage> {
    let image = decode_art(bytes).ok()?;
    let image = image.thumbnail(256, 256).blur(9.0).to_rgba8();
    Some(egui::ColorImage::from_rgba_unmultiplied(
        [image.width() as usize, image.height() as usize],
        image.as_raw(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    async fn serve_artwork(status: &str, bytes: Vec<u8>) -> (String, tokio::task::JoinHandle<()>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/cover", listener.local_addr().unwrap());
        let header = format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            bytes.len()
        );
        let server = tokio::spawn(async move {
            tokio::time::timeout(Duration::from_secs(10), async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    request.push(socket.read_u8().await.unwrap());
                    assert!(request.len() < 8192);
                }
                socket.write_all(header.as_bytes()).await.unwrap();
                // Oversized bodies are rejected before the server finishes.
                let _ = socket.write_all(&bytes).await;
            })
            .await
            .expect("the owned artwork request completes");
        });
        (url, server)
    }

    fn artwork_test_runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    fn artwork_test_loader(runtime: &tokio::runtime::Runtime, dir: PathBuf) -> ArtLoader {
        ArtLoader::new(
            reqwest::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(10))
                .build()
                .unwrap(),
            runtime.handle().clone(),
            dir,
        )
    }

    async fn wait_for_artwork_file(path: &std::path::Path, expected: &[u8]) {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if tokio::fs::read(path)
                    .await
                    .is_ok_and(|bytes| bytes == expected)
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the background writer completes while its runtime is alive");
    }

    #[test]
    fn downloaded_artwork_survives_caller_drop_and_reloads_without_network() {
        let dir =
            std::env::temp_dir().join(format!("fastpotify-art-roundtrip-{}", std::process::id()));
        let runtime = artwork_test_runtime();
        let loader = artwork_test_loader(&runtime, dir.clone());
        let expected: Vec<u8> = (0..256 * 1024).map(|index| (index % 251) as u8).collect();
        let url = runtime.block_on(async {
            let (url, server) = serve_artwork("200 OK", expected.clone()).await;
            let bytes = loader.fetch(&url).await.expect("downloaded artwork");
            assert_eq!(&*bytes, expected.as_slice());
            drop(bytes);
            server.await.unwrap();
            wait_for_artwork_file(&loader.inner.cache_path(&url), &expected).await;
            url
        });
        let path = loader.inner.cache_path(&url);
        drop(loader);
        runtime.shutdown_timeout(Duration::from_secs(10));
        assert_eq!(std::fs::read(&path).unwrap(), expected);
        assert!(!path.with_extension("part").exists());

        let runtime = artwork_test_runtime();
        let loader = artwork_test_loader(&runtime, dir.clone());
        // The server is gone. A new loader must recover the original bytes
        // from the completed cache, without requesting the URL again.
        assert_eq!(&*runtime.block_on(loader.fetch(&url)).unwrap(), expected);
        assert_eq!(loader.cached_file(&url), Some(path));
        drop(loader);
        runtime.shutdown_timeout(Duration::from_secs(10));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn artwork_cache_write_failure_keeps_download_usable() {
        let dir = std::env::temp_dir().join(format!(
            "fastpotify-art-write-failure-{}",
            std::process::id()
        ));
        let runtime = artwork_test_runtime();
        let loader = artwork_test_loader(&runtime, dir.clone());
        let (bytes, path) = runtime.block_on(async {
            let (url, server) = serve_artwork("200 OK", b"complete artwork".to_vec()).await;
            let path = loader.inner.cache_path(&url);
            // A directory cannot be replaced by a file, even by an admin.
            std::fs::create_dir(&path).unwrap();
            let bytes = loader
                .fetch(&url)
                .await
                .expect("display does not depend on caching");
            server.await.unwrap();
            let permit = loader
                .inner
                .fetch_jobs
                .clone()
                .acquire_many_owned(FETCH_JOBS as u32)
                .await
                .unwrap();
            drop(permit);
            (bytes, path)
        });
        drop(loader);
        runtime.shutdown_timeout(Duration::from_secs(10));
        assert_eq!(&*bytes, b"complete artwork");
        assert!(
            path.is_dir(),
            "the failed replacement preserves the old path"
        );
        assert_eq!(
            std::fs::read_dir(&dir).unwrap().count(),
            1,
            "failed writes remove only their own temporary file"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn rejected_artwork_responses_do_not_create_cache_files() {
        let dir =
            std::env::temp_dir().join(format!("fastpotify-art-rejected-{}", std::process::id()));
        let runtime = artwork_test_runtime();
        let loader = artwork_test_loader(&runtime, dir.clone());
        runtime.block_on(async {
            for (status, body, message) in [
                ("404 Not Found", Vec::new(), "artwork request failed: 404"),
                ("200 OK", vec![1; MAX_ART_BYTES + 1], "artwork is too large"),
            ] {
                let (url, server) = serve_artwork(status, body).await;
                let error = loader.fetch(&url).await.unwrap_err();
                assert!(error.starts_with(message), "{error}");
                server.await.unwrap();
            }
        });
        drop(loader);
        runtime.shutdown_timeout(Duration::from_secs(10));
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn transient_backdrop_load_errors_remain_retryable() {
        assert!(!terminal_lyrics_backdrop_error(&LoadError::Loading(
            "temporary network failure".into()
        )));
        assert!(!terminal_lyrics_backdrop_error(
            &LoadError::NoMatchingBytesLoader
        ));
        assert!(terminal_lyrics_backdrop_error(&LoadError::NotSupported));
    }

    #[test]
    fn lyrics_background_rejects_oversized_decode_before_thumbnailing() {
        let image = image::RgbImage::from_pixel(8193, 1, image::Rgb([20, 30, 40]));
        let mut bytes = std::io::Cursor::new(Vec::new());
        image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        assert!(lyrics_background(bytes.get_ref()).is_none());
        assert!(accent_color(bytes.get_ref()).is_none());
        assert!(decode_art(bytes.get_ref()).is_err());
    }

    #[test]
    fn oversized_disk_art_and_streamed_bodies_cannot_bypass_the_byte_limit() {
        let dir = std::env::temp_dir().join(format!("spotidark-art-limits-{}", std::process::id()));
        let runtime = artwork_test_runtime();
        let loader = artwork_test_loader(&runtime, dir.clone());
        let path = loader.inner.cache_path("http://127.0.0.1/too-large");
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(MAX_ART_BYTES as u64 + 1).unwrap();
        assert!(read_cached_art(&path).is_none());
        assert!(loader.cached_file("http://127.0.0.1/too-large").is_none());
        runtime.block_on(async {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}/no-length", listener.local_addr().unwrap());
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    request.push(socket.read_u8().await.unwrap());
                }
                socket
                    .write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n")
                    .await
                    .unwrap();
                let chunk = [7; 16 * 1024];
                for _ in 0..=MAX_ART_BYTES / chunk.len() {
                    if socket.write_all(&chunk).await.is_err() {
                        break;
                    }
                }
            });
            assert_eq!(
                loader.fetch(&url).await.unwrap_err(),
                "artwork is too large"
            );
            server.await.unwrap();
            assert!(!loader.inner.cache_path(&url).exists());
        });
        runtime.shutdown_timeout(Duration::from_secs(10));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn transfer_capacity_is_bounded_and_cancelled_requests_release_it() {
        let dir =
            std::env::temp_dir().join(format!("spotidark-art-concurrency-{}", std::process::id()));
        let runtime = artwork_test_runtime();
        let loader = artwork_test_loader(&runtime, dir.clone());
        runtime.block_on(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let (accepted, mut connections) = tokio::sync::mpsc::unbounded_channel();
            let server = tokio::spawn(async move {
                let mut sockets = Vec::new();
                loop {
                    let (socket, _) = listener.accept().await.unwrap();
                    sockets.push(socket);
                    if accepted.send(()).is_err() {
                        break;
                    }
                }
            });
            let mut requests = tokio::task::JoinSet::new();
            for index in 0..FETCH_JOBS + 2 {
                let loader = loader.clone();
                requests
                    .spawn(async move { loader.fetch(&format!("http://{address}/{index}")).await });
            }
            for _ in 0..FETCH_JOBS {
                tokio::time::timeout(Duration::from_secs(2), connections.recv())
                    .await
                    .unwrap()
                    .unwrap();
            }
            assert!(
                tokio::time::timeout(Duration::from_millis(50), connections.recv())
                    .await
                    .is_err(),
                "additional requests must wait before opening a connection"
            );
            assert_eq!(loader.inner.fetch_jobs.available_permits(), 0);
            requests.abort_all();
            while requests.join_next().await.is_some() {}
            assert_eq!(loader.inner.fetch_jobs.available_permits(), FETCH_JOBS);
            server.abort();
            let _ = server.await;
        });
        runtime.shutdown_timeout(Duration::from_secs(10));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn concurrent_cache_writes_publish_complete_files_and_leave_no_temporaries() {
        let dir =
            std::env::temp_dir().join(format!("spotidark-art-writers-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("cover");
        std::thread::scope(|scope| {
            for byte in 0..4u8 {
                let path = &path;
                scope.spawn(move || write_cached_art(path, &vec![byte; 128 * 1024]));
            }
        });
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(bytes.len(), 128 * 1024);
        assert!(
            bytes.iter().all(|byte| *byte == bytes[0]),
            "a file contains one complete response"
        );
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn byte_reload_preserves_decoded_pixels_and_forgotten_requests_stay_forgotten() {
        let dir =
            std::env::temp_dir().join(format!("spotidark-art-generation-{}", std::process::id()));
        let runtime = artwork_test_runtime();
        let loader = artwork_test_loader(&runtime, dir.clone());
        let ctx = egui::Context::default();
        let url = "https://i.scdn.co/image/reload-pixels";
        let image = Arc::new(egui::ColorImage::filled([4, 4], egui::Color32::RED));
        let mut entry = Entry {
            decoded: Some(Ok(image.clone())),
            raster_bytes: decoded_and_texture_bytes(4, 4),
            ..Entry::default()
        };
        entry.bytes = Some(Arc::from(b"cached art".as_slice()));
        loader
            .inner
            .entries
            .lock()
            .unwrap()
            .insert(url.into(), entry);
        std::fs::write(loader.inner.cache_path(url), b"cached art").unwrap();
        loader.release_bytes(url);
        {
            let mut entries = loader.inner.entries.lock().unwrap();
            entries.get_mut(url).unwrap().error = Some("temporarily offline".into());
        }
        assert!(matches!(loader.load(&ctx, url), Err(LoadError::Loading(_))));
        loader.evict(&ctx);
        runtime.block_on(async {
            assert!(matches!(
                loader.load(&ctx, url),
                Ok(BytesPoll::Pending { .. })
            ));
            tokio::time::timeout(Duration::from_secs(2), async {
                while !matches!(loader.load(&ctx, url), Ok(BytesPoll::Ready { .. })) {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .unwrap();
        });
        let images = loader.image_loader();
        let ImagePoll::Ready { image: after } =
            images.load(&ctx, url, SizeHint::default()).unwrap()
        else {
            panic!("decoded art must survive byte reload")
        };
        assert!(Arc::ptr_eq(&image, &after));
        assert_eq!(
            retained_total(&loader),
            decoded_and_texture_bytes(4, 4) + b"cached art".len()
        );
        loader.forget(url);
        loader
            .inner
            .complete(url, 17, Ok(Arc::from(b"old".as_slice())));
        assert!(!loader.inner.entries.lock().unwrap().contains_key(url));
        loader.inner.entries.lock().unwrap().insert(
            url.into(),
            Entry {
                request: Some(19),
                ..Entry::default()
            },
        );
        loader
            .inner
            .complete(url, 17, Ok(Arc::from(b"old".as_slice())));
        assert!(loader.inner.entries.lock().unwrap()[url].bytes.is_none());
        loader
            .inner
            .complete(url, 19, Ok(Arc::from(b"new".as_slice())));
        assert_eq!(
            loader.inner.entries.lock().unwrap()[url].bytes.as_deref(),
            Some(b"new".as_slice())
        );
        runtime.shutdown_timeout(Duration::from_secs(10));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn decode_gate_keeps_http_rejection_out_of_the_fallback_loader() {
        struct Fallback(Arc<std::sync::atomic::AtomicUsize>);
        impl ImageLoader for Fallback {
            fn id(&self) -> &str {
                "test-fallback"
            }
            fn load(&self, _: &egui::Context, _: &str, _: SizeHint) -> ImageLoadResult {
                self.0.fetch_add(1, Ordering::Relaxed);
                Err(LoadError::NotSupported)
            }
            fn forget(&self, _: &str) {}
            fn forget_all(&self) {}
            fn byte_size(&self) -> usize {
                0
            }
        }
        let dir = std::env::temp_dir().join(format!("spotidark-art-decode-{}", std::process::id()));
        let runtime = artwork_test_runtime();
        let loader = artwork_test_loader(&runtime, dir.clone());
        let ctx = egui::Context::default();
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        ctx.add_image_loader(Arc::new(Fallback(calls.clone())));
        ctx.add_image_loader(Arc::new(loader.image_loader()));
        let url = "https://i.scdn.co/image/malformed";
        loader.inner.entries.lock().unwrap().insert(
            url.into(),
            Entry {
                bytes: Some(Arc::from(b"not an image".as_slice())),
                ..Entry::default()
            },
        );
        runtime.block_on(async {
            let permits = loader
                .inner
                .decode_jobs
                .clone()
                .acquire_many_owned(DECODE_JOBS as u32)
                .await
                .unwrap();
            assert!(matches!(
                ctx.try_load_image(url, SizeHint::default()),
                Ok(ImagePoll::Pending { .. })
            ));
            assert!(
                loader.inner.entries.lock().unwrap()[url].decoding.is_none(),
                "no blocking job may start without capacity"
            );
            drop(permits);
            tokio::time::timeout(Duration::from_secs(2), async {
                loop {
                    match ctx.try_load_image(url, SizeHint::default()) {
                        Err(LoadError::Loading(_)) => break,
                        Ok(ImagePoll::Pending { .. }) => {
                            tokio::time::sleep(Duration::from_millis(5)).await
                        }
                        _ => panic!("unexpected HTTP decode result"),
                    }
                }
            })
            .await
            .unwrap();
        });
        assert_eq!(
            calls.load(Ordering::Relaxed),
            0,
            "invalid HTTP data must never reach the unbounded default decoder"
        );
        let _ = ctx.try_load_image("bytes://local-icon.svg", SizeHint::default());
        assert_eq!(
            calls.load(Ordering::Relaxed),
            1,
            "local icons keep their existing loader chain"
        );
        runtime.shutdown_timeout(Duration::from_secs(10));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn lyrics_background_blurs_edges_and_bounds_texture_size() {
        let image = image::RgbImage::from_fn(640, 320, |x, _| {
            if x < 320 {
                image::Rgb([255, 0, 0])
            } else {
                image::Rgb([0, 0, 255])
            }
        });
        let mut bytes = std::io::Cursor::new(Vec::new());
        image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        let background = lyrics_background(bytes.get_ref()).expect("valid artwork");
        assert_eq!(background.size, [256, 128]);
        let center = background.pixels[64 * 256 + 128];
        assert!(
            center.r() > 40 && center.b() > 40,
            "the edge must be blurred: {center:?}"
        );
        assert!(
            background.pixels[0].r() > 240,
            "the cover's colors remain recognizable"
        );
        assert!(lyrics_background(b"broken artwork").is_none());
    }

    /// The media controls ask for a file rather than a URL, and have to be
    /// told "not yet" rather than handed a path to nothing: macOS loads cover
    /// art itself and dereferences a failed load without checking it, which
    /// takes the whole process with it.
    #[test]
    fn a_cached_file_is_named_only_once_it_is_really_there() {
        let dir = std::env::temp_dir().join(format!("fastpotify-art-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("a runtime to hand the loader");
        let loader = ArtLoader::new(
            reqwest::Client::new(),
            runtime.handle().clone(),
            dir.clone(),
        );
        let url = "https://i.scdn.co/image/abc";

        assert_eq!(loader.cached_file(url), None, "nothing downloaded yet");

        // A half-written download never appears under its real name -- the
        // cache renames one into place -- but an empty file is not artwork.
        let path = loader.inner.cache_path(url);
        std::fs::write(&path, b"").expect("an empty file");
        assert_eq!(loader.cached_file(url), None, "empty is not artwork");

        std::fs::write(&path, b"\xff\xd8\xff jpeg-ish").expect("a file with bytes");
        assert_eq!(loader.cached_file(url), Some(path));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn prefetching_starts_one_download_and_not_another() {
        let dir = std::env::temp_dir().join(format!("fastpotify-prefetch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("a runtime to hand the loader");
        let loader = ArtLoader::new(
            reqwest::Client::new(),
            runtime.handle().clone(),
            dir.clone(),
        );
        let ctx = egui::Context::default();
        let url = "https://i.scdn.co/image/never-drawn";

        assert!(loader.prefetch(&ctx, url), "nobody has asked for it yet");
        assert!(!loader.prefetch(&ctx, url), "it is already on its way");

        // A scheme the loader does not answer for is refused outright, the
        // same as in `load`, and nothing is remembered about it.
        let local = "file:///tmp/cover.jpg";
        assert!(
            !loader.prefetch(&ctx, local),
            "not a URL this loader fetches"
        );
        assert!(
            !loader
                .inner
                .entries
                .lock()
                .expect("the entries")
                .contains_key(local),
            "a URI it cannot fetch was remembered anyway"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn accent_color_finds_dominant_hue() {
        let mut image = image::RgbImage::new(16, 16);
        for (x, _, pixel) in image.enumerate_pixels_mut() {
            *pixel = if x < 12 {
                image::Rgb([20, 120, 200])
            } else {
                image::Rgb([255, 255, 255])
            };
        }
        let mut bytes = Vec::new();
        image
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        let color = accent_color(&bytes).unwrap();
        assert!(
            color[2] > color[0],
            "expected the blue field, got {color:?}"
        );
    }

    fn held(items: &[(&str, u64, usize)]) -> Vec<(String, Instant, usize)> {
        let base = Instant::now();
        items
            .iter()
            .map(|(url, age_secs, bytes)| {
                (
                    (*url).to_string(),
                    base - std::time::Duration::from_secs(*age_secs),
                    *bytes,
                )
            })
            .collect()
    }

    /// Rule: nothing is let go of while it all fits. This is the case
    /// that matters: an evening of listening never reaches the budget,
    /// so no cover ever blinks out and back (#129).
    #[test]
    fn artwork_that_fits_is_all_kept() {
        let art = held(&[("a", 600, 1000), ("b", 300, 1000), ("c", 1, 1000)]);
        assert!(over_budget(art, 10_000).is_empty());
    }

    /// Rule: over the budget, the oldest go first, and only as many as
    /// it takes to fit.
    #[test]
    fn the_oldest_go_until_the_rest_fit() {
        let art = held(&[
            ("oldest", 900, 1000),
            ("middle", 600, 1000),
            ("newest", 1, 1000),
        ]);
        assert_eq!(over_budget(art, 2000), vec!["oldest"]);
    }

    #[test]
    fn enough_go_to_get_under_the_budget() {
        let art = held(&[
            ("oldest", 900, 1000),
            ("middle", 600, 1000),
            ("newest", 1, 1000),
        ]);
        assert_eq!(over_budget(art, 900), vec!["oldest", "middle", "newest"]);
    }

    /// Rule: an empty gallery asks nothing of anyone.
    #[test]
    fn nothing_held_lets_nothing_go() {
        assert!(over_budget(Vec::new(), 0).is_empty());
    }

    fn retained_total(loader: &ArtLoader) -> usize {
        loader
            .inner
            .entries
            .lock()
            .expect("lock")
            .values()
            .map(Entry::retained_bytes)
            .sum()
    }

    #[test]
    fn large_covers_evict_using_decoded_and_texture_sizes() {
        let one = decoded_and_texture_bytes(640, 640);
        assert_eq!(one, 2 * 640 * 640 * 4);
        let jpeg = 50_000usize;
        let dir = std::env::temp_dir().join(format!(
            "fastpotify-art-budget-{}-{}",
            std::process::id(),
            Instant::now().elapsed().as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("a runtime for eviction");
        let loader = ArtLoader::new(
            reqwest::Client::new(),
            runtime.handle().clone(),
            dir.clone(),
        );
        let now = Instant::now();
        for i in 0..40 {
            let url = format!("https://i.scdn.co/image/{i}");
            loader.inner.entries.lock().expect("lock").insert(
                url,
                Entry {
                    bytes: Some(Arc::from(vec![0u8; jpeg])),
                    last_used: now - Duration::from_secs(40 - i),
                    ..Entry::default()
                },
            );
        }
        let jpeg_total = retained_total(&loader);
        assert!(
            jpeg_total < HELD_BYTES,
            "JPEG-only covers must still fit the budget: {jpeg_total}"
        );
        for i in 0..40 {
            let url = format!("https://i.scdn.co/image/{i}");
            loader.release_bytes(&url);
            loader.note_decoded(&url, 640, 640);
        }
        let before = retained_total(&loader);
        assert_eq!(before, 40 * one);
        assert!(
            before > HELD_BYTES,
            "decoded 640×640 covers plus textures must exceed 64 MiB: {before}"
        );
        let ctx = egui::Context::default();
        loader.evict(&ctx);
        let after = retained_total(&loader);
        assert!(
            after <= HELD_BYTES,
            "eviction must bring retained decoded+texture bytes under budget: after={after}"
        );
        assert!(
            after < before,
            "a long scroll of large covers must free memory: before={before} after={after}"
        );
        let entries = loader.inner.entries.lock().expect("lock");
        assert!(
            !entries.contains_key("https://i.scdn.co/image/0"),
            "the oldest scrolled-away cover must go first"
        );
        assert!(
            entries.contains_key("https://i.scdn.co/image/39"),
            "the cover just scrolled into view must stay"
        );
        drop(entries);
        loader.forget_all();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn releasing_bytes_reloads_from_disk_off_the_ui_thread() {
        use egui::load::BytesLoader;
        use std::time::Duration as StdDuration;

        let dir = std::env::temp_dir().join(format!(
            "fastpotify-art-reload-{}-{}",
            std::process::id(),
            Instant::now().elapsed().as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("a runtime for disk reload");
        let loader = ArtLoader::new(
            reqwest::Client::new(),
            runtime.handle().clone(),
            dir.clone(),
        );
        let url = "https://i.scdn.co/image/reload";
        let path = loader.inner.cache_path(url);
        std::fs::create_dir_all(path.parent().expect("cache dir")).expect("cache dir");
        std::fs::write(&path, b"\xff\xd8\xff jpeg-ish").expect("cached jpeg");
        loader.inner.entries.lock().expect("lock").insert(
            url.to_string(),
            Entry {
                bytes: None,
                last_used: Instant::now(),
                ..Entry::default()
            },
        );
        let ctx = egui::Context::default();
        let first = loader.load(&ctx, url).expect("load");
        assert!(
            matches!(first, BytesPoll::Pending { .. }),
            "disk reload must not block the UI thread"
        );
        let deadline = Instant::now() + StdDuration::from_secs(2);
        loop {
            std::thread::sleep(StdDuration::from_millis(20));
            match loader.load(&ctx, url) {
                Ok(BytesPoll::Ready { .. }) => break,
                Ok(BytesPoll::Pending { .. }) if Instant::now() < deadline => continue,
                _ => panic!("reload did not finish"),
            }
        }
        loader.forget_all();
        assert!(loader.inner.entries.lock().expect("lock").is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
