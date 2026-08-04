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
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use notify::Watcher;
use orrery_core::config::Config;
use orrery_core::lookup::Lookup;
use orrery_core::scene::Scene;
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
                options.size = (width.parse()?, height.parse()?);
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

/// Hand the star catalogue to the renderer, tolerating a broken one.
fn load_catalog_into(renderer: &mut Renderer, device: &wgpu::Device, config: &Config) {
    if !config.sky.real_stars && !config.sky.constellations {
        return;
    }
    match orrery_core::sky::Catalog::embedded() {
        Ok(catalog) => renderer.set_catalog(device, &catalog, config),
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
}

impl App {
    fn new(
        config: Config,
        config_path: Option<PathBuf>,
        options: Options,
        lookup: Lookup,
    ) -> Self {
        let epoch = config.time.start_epoch().unwrap_or_else(|_| JulianDate::now());
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

    /// Adopt a completed background fetch, if there is one.
    fn collect_refresh(&mut self) {
        let Some(rx) = &self.almanac_rx else { return };
        match rx.try_recv() {
            Ok(Some(almanac)) => {
                log::info!(
                    "ephemeris refreshed: {} bodies from JPL Horizons",
                    almanac.bodies.len()
                );
                self.lookup.set_almanac(Some(almanac));
                self.almanac_rx = None;
            }
            Ok(None) => self.almanac_rx = None,
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => self.almanac_rx = None,
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
                self.epoch = config.time.start_epoch().unwrap_or_else(|_| JulianDate::now());
                self.started = Instant::now();
                self.config = config;
            }
            // A half-written file mid-save is normal; keep the old config.
            Err(error) => log::warn!("ignoring invalid configuration: {error:#}"),
        }
    }

    fn draw(&mut self) {
        // Sample the clock before borrowing `graphics` mutably.
        let epoch = self.current_epoch();
        let Some(graphics) = &mut self.graphics else { return };

        let size = graphics.window.inner_size();
        if size.width == 0 || size.height == 0 {
            return;
        }

        let aspect = size.width as f32 / size.height as f32;
        let elapsed = self.started.elapsed().as_secs_f32();

        // The camera drift is expressed per hour, so it stays gentle.
        let mut config = self.config.clone();
        if config.camera.rotation_period_minutes > 0.0 {
            let period = config.camera.rotation_period_minutes * 60.0;
            config.camera.azimuth_deg += (elapsed as f64 / period * 360.0) as f32;
        }
        let scene = Scene::build(&config, &self.lookup, epoch, aspect);

        let frame = match graphics.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                graphics
                    .surface
                    .configure(&graphics.device, &graphics.surface_config);
                return;
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => return,
            wgpu::CurrentSurfaceTexture::Validation => {
                log::error!("surface validation error");
                return;
            }
        };

        let view = frame.texture.create_view(&Default::default());
        graphics.renderer.render(
            &graphics.device,
            &graphics.queue,
            &view,
            &scene,
            &config,
            elapsed,
        );
        graphics.queue.present(frame);
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
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
            WindowEvent::RedrawRequested => self.draw(),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.reload_config_if_changed();
        self.collect_refresh();

        // A wallpaper has no business running at the display's full refresh
        // rate, so redraws are paced to the configured frame budget.
        let budget = Duration::from_secs_f64(1.0 / self.config.render.fps.max(1) as f64);
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

impl App {
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
            power_preference: wgpu::PowerPreference::HighPerformance,
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

        let size = window.inner_size();
        let (width, height) = (size.width.max(1), size.height.max(1));

        let capabilities = surface.get_capabilities(&adapter);
        // Prefer an sRGB target so the hardware applies the transfer function.
        let format = capabilities
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(capabilities.formats[0]);

        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            color_space: wgpu::SurfaceColorSpace::Auto,
            width,
            height,
            present_mode: if self.config.render.vsync {
                wgpu::PresentMode::Fifo
            } else {
                wgpu::PresentMode::AutoNoVsync
            },
            desired_maximum_frame_latency: 2,
            alpha_mode: wgpu::CompositeAlphaMode::Opaque,
            view_formats: vec![],
        };
        surface.configure(&device, &surface_config);

        let mut renderer = Renderer::new(&device, format, width, height, &self.config);
        load_catalog_into(&mut renderer, &device, &self.config);

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
                && event.paths.iter().any(|p| *p == watched)
            {
                let _ = tx.send(());
            }
        })?;
    watcher.watch(&directory, notify::RecursiveMode::NonRecursive)?;
    Ok((rx, watcher))
}
