//! The orrery binary.
//!
//! Deliberately an ordinary window rather than anything wallpaper-specific.
//! On KDE Plasma the desktop is itself a layer-shell surface that plasmashell
//! paints opaque, so a third-party background-layer surface lands *on top* of
//! the desktop icons rather than behind them. The supported way in is to be a
//! plain client that a Plasma wallpaper plugin hosts and composites — see
//! `plasma/` and the README. That also means the same binary runs as a normal
//! window for development, and as a desktop-level window on macOS.

mod horizons;
mod platform;
mod screenshot;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use notify::Watcher;
use orrery_core::config::Config;
use orrery_core::lookup::Lookup;
use orrery_core::scene::{Scene, SceneCache};
use orrery_core::time::JulianDate;
use orrery_render::Renderer;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

/// How the binary was asked to run.
struct Options {
    config_path: Option<PathBuf>,
    windowed: bool,
    screenshot: Option<PathBuf>,
    size: (u32, u32),
    /// Fetch a fresh almanac synchronously, report, and exit.
    refresh_ephemeris: bool,
    /// Validate the configuration, report, and exit.
    check_config: bool,
    /// Where `--refresh-ephemeris` writes. Defaults to the user cache path.
    ephemeris_out: Option<PathBuf>,
}

fn main() -> Result<()> {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("orrery=info,wgpu=warn"),
    )
    .init();

    let options = parse_arguments()?;
    let config_path = options
        .config_path
        .clone()
        .or_else(default_config_path);
    // Validate before anything else, so a stale config is a clear message
    // rather than a failure inside the renderer.
    if options.check_config {
        return match load_config(config_path.as_deref()) {
            Ok(_) => {
                println!("configuration is valid");
                Ok(())
            }
            Err(error) => {
                eprintln!("{error:#}");
                std::process::exit(1);
            }
        };
    }

    let config = load_config(config_path.as_deref())?;

    if options.refresh_ephemeris {
        return refresh_ephemeris_now(options.ephemeris_out.clone());
    }

    let lookup = load_lookup(&config);

    if let Some(path) = &options.screenshot {
        return screenshot::capture(&config, &lookup, options.size, path);
    }

    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App::new(config, config_path, options, lookup);
    event_loop.run_app(&mut app)?;
    Ok(())
}

fn parse_arguments() -> Result<Options> {
    let mut options = Options {
        config_path: None,
        windowed: false,
        screenshot: None,
        size: (1920, 1080),
        refresh_ephemeris: false,
        check_config: false,
        ephemeris_out: None,
    };

    let mut args = std::env::args().skip(1);
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--config" | "-c" => {
                options.config_path = Some(
                    args.next()
                        .context("--config needs a path")?
                        .into(),
                );
            }
            "--windowed" | "-w" => options.windowed = true,
            "--check-config" => options.check_config = true,
            "--refresh-ephemeris" => options.refresh_ephemeris = true,
            "--ephemeris-out" => {
                options.ephemeris_out = Some(
                    args.next()
                        .context("--ephemeris-out needs a path")?
                        .into(),
                );
            }
            "--screenshot" => {
                options.screenshot = Some(
                    args.next()
                        .context("--screenshot needs an output path")?
                        .into(),
                );
            }
            "--size" => {
                let value = args.next().context("--size needs WIDTHxHEIGHT")?;
                let (width, height) = value
                    .split_once(['x', 'X'])
                    .context("--size must look like 1920x1080")?;
                options.size = (
                    width
                        .parse()
                        .with_context(|| format!("--size width {width:?} is not a number"))?,
                    height
                        .parse()
                        .with_context(|| format!("--size height {height:?} is not a number"))?,
                );
            }
            "--help" | "-h" => {
                println!(
                    "orrery — an accurate, animated solar system for your desktop\n\n\
                     Usage: orrery [options]\n\n\
                     Options:\n  \
                     -c, --config <PATH>     configuration file to use\n  \
                     -w, --windowed          run in a normal resizable window\n      \
                     --screenshot <PATH>     render a single PNG frame and exit\n      \
                     --size <WxH>            size for --screenshot (default 1920x1080)\n      \
                     --check-config          report whether the configuration is valid and exit\n      \
                     --refresh-ephemeris     fetch fresh elements from JPL Horizons and exit\n      \
                     --ephemeris-out <PATH>  where --refresh-ephemeris writes\n  \
                     -h, --help              show this message\n\n\
                     With no --config, reads $XDG_CONFIG_HOME/orrery/orrery.toml\n\
                     (or ~/.config/orrery/orrery.toml), falling back to built-in defaults."
                );
                std::process::exit(0);
            }
            other => bail!("unrecognised argument {other:?}; try --help"),
        }
    }
    Ok(options)
}

fn default_config_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(base.join("orrery").join("orrery.toml"))
}

/// Load a config, tolerating a missing file but not a malformed one.
fn load_config(path: Option<&std::path::Path>) -> Result<Config> {
    let Some(path) = path else {
        return Ok(Config::default());
    };
    match std::fs::read_to_string(path) {
        Ok(text) => {
            let config = Config::from_toml(&text)
                .with_context(|| format!("in {}", path.display()))?;
            log::info!("loaded configuration from {}", path.display());
            Ok(config)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            log::info!(
                "no configuration at {}, using defaults",
                path.display()
            );
            Ok(Config::default())
        }
        Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
    }
}


/// Fetch a fresh almanac synchronously and write it out. This is the path a
/// cron job or systemd timer would use, and how the bundled almanac is made.
fn refresh_ephemeris_now(destination: Option<PathBuf>) -> Result<()> {
    let path = destination
        .or_else(horizons::cache_path)
        .context("no writable location for the almanac")?;
    let now = JulianDate::now();
    log::info!("fetching osculating elements from JPL Horizons");
    let almanac = horizons::fetch(now)?;
    horizons::save(&almanac, &path)?;
    let epochs = almanac
        .sets_for(orrery_core::Planet::Earth)
        .map_or(0, |sets| sets.len());
    log::info!(
        "wrote {} ({} bodies, {epochs} epochs each)",
        path.display(),
        almanac.bodies.len()
    );
    Ok(())
}

/// An almanac fetched at build time, so a fresh install is accurate straight
/// away and stays accurate with the network permanently disabled. Once it stops
/// covering the present it simply stops being used and the built-in tables take
/// over, which is also what triggers a refresh.
const BUNDLED_ALMANAC: &str = include_str!("../../../data/almanac.toml");

/// How often a healthy process re-asks whether the almanac needs refreshing.
/// A wallpaper runs for months, so waiting to be restarted is not a schedule.
const REFRESH_CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// First retry delay after a failed fetch. Doubles per failure up to
/// [`REFRESH_CHECK_INTERVAL`], so a laptop that was briefly offline recovers
/// within the hour while a genuinely dead network settles at one try a day.
const REFRESH_RETRY_MIN: Duration = Duration::from_secs(60 * 60);

/// The retry delay that follows a failed fetch at the given delay.
fn next_retry_backoff(current: Duration) -> Duration {
    (current * 2).min(REFRESH_CHECK_INTERVAL)
}

/// Hand the star catalogue to the renderer, tolerating a broken one.
fn load_catalog_into(renderer: &mut Renderer, device: &wgpu::Device) {
    match orrery_core::sky::Catalog::embedded() {
        Ok(catalog) => renderer.set_catalog(device, &catalog),
        // The procedural sky still works, so this is not fatal.
        Err(error) => log::warn!("could not load the star catalogue: {error}"),
    }
}

/// Build the position source, best first: the cached almanac, then the bundled
/// one, then the built-in tables.
fn load_lookup(config: &Config) -> Lookup {
    if !config.ephemeris.online {
        log::info!("ephemeris look-up disabled by config; using the built-in tables");
        return Lookup::builtin();
    }

    let cached = horizons::cache_path().and_then(|path| horizons::load_cached(&path));
    if let Some(almanac) = cached {
        return Lookup::with_almanac(almanac);
    }

    match orrery_core::almanac::Almanac::from_toml(BUNDLED_ALMANAC) {
        Ok(almanac) => {
            log::info!("using the bundled almanac until a refresh completes");
            Lookup::with_almanac(almanac)
        }
        Err(error) => {
            log::warn!("the bundled almanac is unusable: {error}");
            Lookup::builtin()
        }
    }
}

/// GPU state, which only exists once there is a window to draw into.
struct Graphics {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    surface_config: wgpu::SurfaceConfiguration,
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: Renderer,
}

struct App {
    config: Config,
    config_path: Option<PathBuf>,
    options: Options,
    graphics: Option<Graphics>,
    epoch: JulianDate,
    started: Instant,
    last_frame: Instant,
    /// Config-file change notifications. Held so the watcher stays alive.
    reload_rx: Option<Receiver<()>>,
    _watcher: Option<notify::RecommendedWatcher>,
    /// Where positions come from. Swapped in place when a refresh lands.
    lookup: Lookup,
    /// Result of the background Horizons fetch, if one is in flight.
    almanac_rx: Option<Receiver<Option<orrery_core::almanac::Almanac>>>,
    /// When to next ask whether the almanac needs refreshing.
    next_refresh_check: Instant,
    /// Retry delay for the next attempt after a failed fetch.
    refresh_backoff: Duration,
    /// Set from the wgpu device-lost callback, which may fire on any thread;
    /// the event loop reacts by rebuilding the graphics on its own thread.
    device_lost: Arc<AtomicBool>,
    /// Belts and orbit geometry reused across frames.
    scene_cache: SceneCache,
    /// When the surface started reporting `Occluded`, if it currently does.
    /// Drives the slow-tick backoff and, after a few seconds, the release of
    /// the render targets' VRAM.
    occluded_since: Option<Instant>,
}

/// How long the surface must stay occluded before the render targets are
/// released. Long enough that alt-tabbing does not thrash hundreds of
/// megabytes of allocations; short enough that a game gets the memory back
/// moments after it covers the desktop.
const OCCLUDED_RELEASE_AFTER: Duration = Duration::from_secs(5);

/// The frame budget while occluded: one probe a second to notice becoming
/// visible again, instead of the full configured rate for invisible frames.
const OCCLUDED_FRAME_BUDGET: Duration = Duration::from_secs(1);

impl App {
    fn new(
        config: Config,
        config_path: Option<PathBuf>,
        options: Options,
        lookup: Lookup,
    ) -> Self {
        let epoch = config.time.start_epoch().unwrap_or_else(|error| {
            log::warn!("ignoring unusable [time] date: {error}");
            JulianDate::now()
        });
        let (reload_rx, watcher) = match &config_path {
            Some(path) => match watch_config(path) {
                Ok((rx, watcher)) => (Some(rx), Some(watcher)),
                Err(error) => {
                    log::warn!("config hot-reload unavailable: {error}");
                    (None, None)
                }
            },
            None => (None, None),
        };

        let mut app = Self {
            config,
            config_path,
            options,
            graphics: None,
            epoch,
            started: Instant::now(),
            last_frame: Instant::now(),
            reload_rx,
            _watcher: watcher,
            lookup,
            almanac_rx: None,
            next_refresh_check: Instant::now() + REFRESH_CHECK_INTERVAL,
            refresh_backoff: REFRESH_RETRY_MIN,
            device_lost: Arc::new(AtomicBool::new(false)),
            scene_cache: SceneCache::new(),
            occluded_since: None,
        };
        app.start_refresh_if_due();
        app
    }

    /// Kick off a Horizons fetch on a background thread if the almanac is
    /// missing or stale.
    ///
    /// Nothing here can delay a frame: the render loop keeps drawing from
    /// whatever source it already has, and picks up the result when it lands.
    fn start_refresh_if_due(&mut self) {
        if !self.config.ephemeris.online || self.almanac_rx.is_some() {
            return;
        }
        let now = JulianDate::now();
        if !horizons::needs_refresh(self.lookup.almanac(), now, self.config.ephemeris.refresh_days)
        {
            return;
        }

        let (tx, rx) = channel();
        self.almanac_rx = Some(rx);
        std::thread::spawn(move || {
            let result = match horizons::fetch(now) {
                Ok(almanac) => {
                    if let Some(path) = horizons::cache_path()
                        && let Err(error) = horizons::save(&almanac, &path)
                    {
                        log::warn!("could not cache the almanac: {error:#}");
                    }
                    Some(almanac)
                }
                Err(error) => {
                    // Offline, blocked, or JPL is down. The built-in tables
                    // remain perfectly serviceable.
                    log::warn!("ephemeris refresh failed, keeping current source: {error:#}");
                    None
                }
            };
            let _ = tx.send(result);
        });
        log::info!("refreshing the ephemeris in the background");
    }

    /// Adopt a completed background fetch, if there is one. A failed fetch
    /// schedules a retry with growing backoff instead of giving up until the
    /// process is restarted — which, for a wallpaper, may be never.
    fn collect_refresh(&mut self) {
        let Some(rx) = &self.almanac_rx else { return };
        match rx.try_recv() {
            Ok(Some(almanac)) => {
                log::info!(
                    "ephemeris refreshed: {} bodies from JPL Horizons",
                    almanac.bodies.len()
                );
                self.lookup.set_almanac(Some(almanac));
                // The cached orbit geometry was computed from the old source.
                self.scene_cache.invalidate();
                self.almanac_rx = None;
                self.next_refresh_check = Instant::now() + REFRESH_CHECK_INTERVAL;
                self.refresh_backoff = REFRESH_RETRY_MIN;
            }
            Ok(None) => {
                self.almanac_rx = None;
                self.next_refresh_check = Instant::now() + self.refresh_backoff;
                self.refresh_backoff = next_retry_backoff(self.refresh_backoff);
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                // The fetch thread died without reporting; treat it as a
                // failure so the schedule keeps moving.
                self.almanac_rx = None;
                self.next_refresh_check = Instant::now() + self.refresh_backoff;
                self.refresh_backoff = next_retry_backoff(self.refresh_backoff);
            }
        }
    }

    /// The instant to draw, given how long we have been running.
    fn current_epoch(&self) -> JulianDate {
        use orrery_core::config::TimeMode;
        let elapsed = self.started.elapsed().as_secs_f64();
        match self.config.time.mode {
            TimeMode::Fixed => self.epoch,
            TimeMode::Live if self.config.time.days_per_second == 0.0 => {
                // Genuinely live: the real solar system, right now.
                match self.config.time.date {
                    Some(_) => self.epoch,
                    None => JulianDate::now(),
                }
            }
            TimeMode::Live => {
                JulianDate(self.epoch.0 + elapsed * self.config.time.days_per_second)
            }
        }
    }

    fn reload_config_if_changed(&mut self) {
        let Some(rx) = &self.reload_rx else { return };
        // Editors emit several events per save; collapse them.
        let mut changed = false;
        while rx.try_recv().is_ok() {
            changed = true;
        }
        if !changed {
            return;
        }
        match load_config(self.config_path.as_deref()) {
            Ok(config) => {
                log::info!("configuration reloaded");
                // Only reset the clocks when the time base actually changed;
                // otherwise every save of the file — even a comment edit —
                // snaps the camera drift back to zero.
                if config.time != self.config.time {
                    self.epoch = config.time.start_epoch().unwrap_or_else(|error| {
                        log::warn!("ignoring unusable [time] date: {error}");
                        JulianDate::now()
                    });
                    self.started = Instant::now();
                }
                if config.render.vsync != self.config.render.vsync
                    && let Some(graphics) = &mut self.graphics
                {
                    graphics.surface_config.present_mode = present_mode(config.render.vsync);
                    graphics
                        .surface
                        .configure(&graphics.device, &graphics.surface_config);
                }
                self.config = config;
            }
            // A half-written file mid-save is normal; keep the old config.
            Err(error) => log::warn!("ignoring invalid configuration: {error:#}"),
        }
    }

    fn draw(&mut self) {
        // Sample the clock before borrowing `graphics` mutably.
        let epoch = self.current_epoch();
        let elapsed = self.started.elapsed().as_secs_f64();
        let Some(graphics) = &mut self.graphics else { return };

        let size = graphics.window.inner_size();
        if size.width == 0 || size.height == 0 {
            return;
        }

        // Acquire the frame *before* building the scene, so a covered or
        // occluded wallpaper skips the whole CPU build, not just the GPU work.
        let mut suboptimal = false;
        let frame = match graphics.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame) => frame,
            wgpu::CurrentSurfaceTexture::Suboptimal(frame) => {
                // Usable this frame, but reconfigure after presenting so the
                // swapchain does not stay suboptimal indefinitely.
                suboptimal = true;
                frame
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                graphics
                    .surface
                    .configure(&graphics.device, &graphics.surface_config);
                return;
            }
            wgpu::CurrentSurfaceTexture::Timeout => return,
            wgpu::CurrentSurfaceTexture::Occluded => {
                // Invisible. After a grace period, hand the render targets'
                // VRAM back so a fullscreen application is not competing with
                // a wallpaper it has covered.
                let since = *self.occluded_since.get_or_insert_with(Instant::now);
                if since.elapsed() >= OCCLUDED_RELEASE_AFTER {
                    graphics.renderer.release_targets();
                }
                return;
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                log::error!("surface validation error");
                return;
            }
        };
        self.occluded_since = None;

        // The acquired frame is the authoritative size: the window's inner
        // size can disagree for a frame around a resize, and the renderer's
        // projection uses the surface dimensions.
        let aspect = frame.texture.width() as f32 / frame.texture.height() as f32;

        // The camera drift is expressed per hour, so it stays gentle. It is
        // handed to the scene as a parameter — mutating a clone of the config
        // every frame would both allocate and destabilise the scene cache.
        let camera = &self.config.camera;
        let azimuth_deg = if camera.rotation_period_minutes > 0.0 {
            let period = camera.rotation_period_minutes * 60.0;
            // Accumulate in f64 and wrap before the one cast: an f32 total in
            // the hundreds of thousands of degrees — a few weeks of uptime —
            // has ULPs coarser than the drift it is accumulating.
            (f64::from(camera.azimuth_deg) + elapsed / period * 360.0).rem_euclid(360.0) as f32
        } else {
            camera.azimuth_deg
        };
        let scene = Scene::build_cached(
            &self.config,
            &self.lookup,
            epoch,
            aspect,
            azimuth_deg,
            &mut self.scene_cache,
        );

        let view = frame.texture.create_view(&Default::default());
        graphics.renderer.render(
            &graphics.device,
            &graphics.queue,
            &view,
            &scene,
            &self.config,
            elapsed as f32,
        );
        graphics.queue.present(frame);

        if suboptimal {
            graphics
                .surface
                .configure(&graphics.device, &graphics.surface_config);
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.ensure_graphics(event_loop);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        logical_key: Key::Named(NamedKey::Escape),
                        state: ElementState::Pressed,
                        ..
                    },
                ..
            } => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(graphics) = &mut self.graphics
                    && size.width > 0
                    && size.height > 0
                {
                    graphics.surface_config.width = size.width;
                    graphics.surface_config.height = size.height;
                    graphics
                        .surface
                        .configure(&graphics.device, &graphics.surface_config);
                    graphics
                        .renderer
                        .resize(&graphics.device, size.width, size.height);
                }
            }
            // A DPI change alone does not always come with a `Resized`, and a
            // stale surface size on the new scale renders blurry.
            WindowEvent::ScaleFactorChanged { .. } => {
                if let Some(graphics) = &mut self.graphics {
                    let size = graphics.window.inner_size();
                    if size.width > 0 && size.height > 0 {
                        graphics.surface_config.width = size.width;
                        graphics.surface_config.height = size.height;
                        graphics
                            .surface
                            .configure(&graphics.device, &graphics.surface_config);
                        graphics
                            .renderer
                            .resize(&graphics.device, size.width, size.height);
                    }
                }
            }
            WindowEvent::RedrawRequested => self.draw(),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.reload_config_if_changed();
        self.collect_refresh();

        // A lost device — driver reset, GPU gone to sleep badly — leaves every
        // resource invalid. The callback only sets a flag, because it can fire
        // on any thread; the rebuild has to happen here, on the loop's thread.
        if self.device_lost.swap(false, Ordering::Relaxed) {
            log::warn!("GPU device lost; rebuilding the renderer");
            self.graphics = None;
            self.ensure_graphics(event_loop);
        }

        // Re-ask about the almanac on schedule. `needs_refresh` remains the
        // staleness authority, so this costs one comparison a frame.
        if self.almanac_rx.is_none() && Instant::now() >= self.next_refresh_check {
            self.next_refresh_check = Instant::now() + REFRESH_CHECK_INTERVAL;
            self.start_refresh_if_due();
        }

        // A wallpaper has no business running at the display's full refresh
        // rate, so redraws are paced to the configured frame budget — and an
        // occluded one only probes for visibility, at one frame a second.
        let budget = if self.occluded_since.is_some() {
            OCCLUDED_FRAME_BUDGET
        } else {
            Duration::from_secs_f64(1.0 / self.config.render.fps.max(1) as f64)
        };
        let since_last = self.last_frame.elapsed();
        if since_last >= budget {
            self.last_frame = Instant::now();
            if let Some(graphics) = &self.graphics {
                graphics.window.request_redraw();
            }
            event_loop.set_control_flow(ControlFlow::Poll);
        } else {
            event_loop.set_control_flow(ControlFlow::WaitUntil(
                Instant::now() + (budget - since_last),
            ));
        }
    }
}

/// The present mode for a vsync choice, shared by creation and reload.
fn present_mode(vsync: bool) -> wgpu::PresentMode {
    if vsync {
        wgpu::PresentMode::Fifo
    } else {
        wgpu::PresentMode::AutoNoVsync
    }
}

impl App {
    /// Create the window and GPU state if they do not currently exist.
    /// Called at startup and again after a device loss.
    fn ensure_graphics(&mut self, event_loop: &ActiveEventLoop) {
        if self.graphics.is_some() {
            return;
        }
        match self.create_graphics(event_loop) {
            Ok(graphics) => self.graphics = Some(graphics),
            Err(error) => {
                log::error!("could not start the renderer: {error:#}");
                event_loop.exit();
            }
        }
    }

    fn create_graphics(&self, event_loop: &ActiveEventLoop) -> Result<Graphics> {
        let mut attributes = Window::default_attributes()
            .with_title("Orrery")
            .with_inner_size(winit::dpi::LogicalSize::new(1280.0, 720.0));
        if !self.options.windowed {
            attributes = attributes.with_decorations(false);
        }
        let window = Arc::new(event_loop.create_window(attributes)?);
        platform::configure_window(&window, self.options.windowed);

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::from_env().unwrap_or(wgpu::Backends::PRIMARY),
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });

        let surface = instance.create_surface(window.clone())?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            // A wallpaper on the integrated GPU, always. `HighPerformance`
            // pins the discrete GPU of a hybrid machine awake around the
            // clock to draw a background; the workload here is trivial for
            // any adapter. The screenshot path keeps `HighPerformance` — a
            // one-shot render is exactly what it is for.
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
            ..Default::default()
        }))
        .context("no suitable GPU adapter")?;
        log::info!("using {}", adapter.get_info().name);

        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("orrery"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                ..Default::default()
            }))?;

        // Errors outside an error scope would otherwise panic wgpu's internal
        // thread; a wallpaper should log and keep drawing what it can.
        device.on_uncaptured_error(Arc::new(|error| {
            log::error!("wgpu error: {error}");
        }));
        // The callback may fire on any thread, so it only raises a flag; the
        // event loop notices and rebuilds. This is the only path through a
        // driver reset for a process that runs for weeks.
        let device_lost = self.device_lost.clone();
        device.set_device_lost_callback(move |reason, message| {
            log::warn!("device lost ({reason:?}): {message}");
            device_lost.store(true, Ordering::Relaxed);
        });

        let size = window.inner_size();
        let (width, height) = (size.width.max(1), size.height.max(1));

        let capabilities = surface.get_capabilities(&adapter);
        // Prefer an sRGB target so the hardware applies the transfer function.
        let format = capabilities
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .or_else(|| capabilities.formats.first().copied())
            .context("the surface reports no supported formats")?;
        if !format.is_srgb() {
            // Tonemapping dithers in linear space expecting the hardware to
            // apply the sRGB transfer function; without it the output is
            // visibly wrong, but still better than refusing to start.
            log::warn!("no sRGB surface format available; using {format:?} and colours will be off");
        }
        // `Opaque` is not universal; fall back to whatever the compositor
        // advertises rather than failing surface configuration.
        let alpha_mode = if capabilities
            .alpha_modes
            .contains(&wgpu::CompositeAlphaMode::Opaque)
        {
            wgpu::CompositeAlphaMode::Opaque
        } else {
            wgpu::CompositeAlphaMode::Auto
        };

        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            color_space: wgpu::SurfaceColorSpace::Auto,
            width,
            height,
            present_mode: present_mode(self.config.render.vsync),
            desired_maximum_frame_latency: 2,
            alpha_mode,
            view_formats: vec![],
        };
        surface.configure(&device, &surface_config);

        let mut renderer = Renderer::new(&device, format, width, height);
        load_catalog_into(&mut renderer, &device);

        Ok(Graphics {
            window,
            surface,
            surface_config,
            device,
            queue,
            renderer,
        })
    }
}

/// Watch the config file and signal on change.
///
/// The parent directory is watched rather than the file itself, because editors
/// typically save by writing a temporary file and renaming it over the target,
/// which destroys any watch held on the original inode.
fn watch_config(path: &std::path::Path) -> Result<(Receiver<()>, notify::RecommendedWatcher)> {
    let (tx, rx) = channel();
    let directory = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| std::path::Path::new("."))
        .to_path_buf();
    let watched = path.to_path_buf();

    let mut watcher =
        notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            if let Ok(event) = event
                && event.paths.contains(&watched)
            {
                let _ = tx.send(());
            }
        })?;
    watcher.watch(&directory, notify::RecursiveMode::NonRecursive)?;
    Ok((rx, watcher))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_backoff_doubles_and_caps_at_the_daily_check() {
        let mut delay = REFRESH_RETRY_MIN;
        let mut previous = Duration::ZERO;
        // However many failures arrive, the schedule keeps moving forward and
        // settles at one attempt per day rather than growing without bound.
        for _ in 0..10 {
            assert!(delay > previous, "backoff must not shrink");
            assert!(delay <= REFRESH_CHECK_INTERVAL, "backoff must cap at the daily check");
            previous = delay;
            let next = next_retry_backoff(delay);
            if next == delay {
                break;
            }
            delay = next;
        }
        assert_eq!(delay, REFRESH_CHECK_INTERVAL, "the cap is the daily check interval");
        assert_eq!(next_retry_backoff(REFRESH_CHECK_INTERVAL), REFRESH_CHECK_INTERVAL);
    }
}
