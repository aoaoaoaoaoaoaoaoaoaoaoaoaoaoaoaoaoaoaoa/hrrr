//! Desktop-presence boundary. X11 speaks `XEmbed` directly because `AppIndicator`
//! discards activation events; macOS and Windows use Tauri's native tray core.

#[derive(Clone, Copy, Debug)]
pub enum Signal {
    Reveal,
    Quit,
}

#[cfg(target_os = "linux")]
mod platform {
    use super::Signal;
    use anyhow::{Context as _, Result};
    use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
    use std::{
        io::Write as _,
        os::{fd::AsFd as _, unix::net::UnixStream},
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        thread::{self, JoinHandle},
        time::{Duration, Instant},
    };
    use x11rb::{
        CURRENT_TIME, NONE,
        connection::Connection,
        protocol::{
            Event,
            randr::ConnectionExt as _,
            xproto::{
                Arc as XArc, Atom, AtomEnum, ButtonPressEvent, CapStyle, ClientMessageEvent,
                ConnectionExt as _, CoordMode, CreateGCAux, CreateWindowAux, EventMask, Gcontext,
                GrabMode, JoinStyle, LineStyle, Point, PropMode, Window, WindowClass,
            },
        },
        rust_connection::RustConnection,
        wrapper::ConnectionExt as _,
    };

    const ICON_SIZE: u16 = 24;
    const MENU_WIDTH: u16 = 104;
    const MENU_HEIGHT: u16 = 30;
    const MENU_BORDER: u16 = 1;
    const DOCK_REQUEST: u32 = 0;
    const XEMBED_MAPPED: u32 = 1;
    const OWNER_POLL: Duration = Duration::from_millis(500);
    const MENU_GAP: i32 = 4;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct DesktopRect {
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    }

    impl DesktopRect {
        fn right(self) -> i32 {
            self.x + self.width
        }

        fn bottom(self) -> i32 {
            self.y + self.height
        }

        fn distance_squared(self, [x, y]: [i32; 2]) -> i64 {
            let nearest_x = x.clamp(self.x, self.right() - 1);
            let nearest_y = y.clamp(self.y, self.bottom() - 1);
            i64::from(x - nearest_x).pow(2) + i64::from(y - nearest_y).pow(2)
        }
    }

    fn nearest_monitor(monitors: &[DesktopRect], point: [i32; 2]) -> Option<DesktopRect> {
        monitors
            .iter()
            .copied()
            .filter(|monitor| monitor.width > 0 && monitor.height > 0)
            .min_by_key(|monitor| monitor.distance_squared(point))
    }

    fn popup_origin(
        monitor: DesktopRect,
        anchor: DesktopRect,
        [width, height]: [i32; 2],
    ) -> [i32; 2] {
        let x = (anchor.right() - width).clamp(monitor.x, (monitor.right() - width).max(monitor.x));
        let below = anchor.bottom() + MENU_GAP;
        let above = anchor.y - MENU_GAP - height;
        let desired_y = if below + height <= monitor.bottom() {
            below
        } else {
            above
        };
        let y = desired_y.clamp(monitor.y, (monitor.bottom() - height).max(monitor.y));
        [x, y]
    }

    struct Atoms {
        selection: Atom,
        opcode: Atom,
        xembed_info: Atom,
    }

    struct X11Tray {
        conn: RustConnection,
        root: Window,
        root_width: u16,
        root_height: u16,
        icon: Window,
        menu: Window,
        contour_gc: Gcontext,
        ember_gc: Gcontext,
        menu_gc: Gcontext,
        atoms: Atoms,
        owner: Window,
        menu_live: bool,
        available: Arc<AtomicBool>,
        emit: Arc<dyn Fn(Signal) + Send + Sync>,
    }

    impl X11Tray {
        fn forge(
            available: Arc<AtomicBool>,
            emit: Arc<dyn Fn(Signal) + Send + Sync>,
        ) -> Result<Self> {
            let (conn, screen_number) = RustConnection::connect(None).context("connect to X11")?;
            let screen = &conn.setup().roots[screen_number];
            let root = screen.root;
            let (root_width, root_height, depth, visual, colormap) = (
                screen.width_in_pixels,
                screen.height_in_pixels,
                screen.root_depth,
                screen.root_visual,
                screen.default_colormap,
            );
            let atoms = Atoms {
                selection: intern(&conn, &format!("_NET_SYSTEM_TRAY_S{screen_number}"))?,
                opcode: intern(&conn, "_NET_SYSTEM_TRAY_OPCODE")?,
                xembed_info: intern(&conn, "_XEMBED_INFO")?,
            };
            let icon = conn.generate_id().context("allocate tray icon window")?;
            conn.create_window(
                depth,
                icon,
                root,
                0,
                0,
                ICON_SIZE,
                ICON_SIZE,
                0,
                WindowClass::INPUT_OUTPUT,
                visual,
                &CreateWindowAux::new()
                    .background_pixmap(x11rb::protocol::xproto::BackPixmap::PARENT_RELATIVE)
                    .event_mask(
                        EventMask::EXPOSURE | EventMask::BUTTON_PRESS | EventMask::STRUCTURE_NOTIFY,
                    ),
            )?
            .check()
            .context("create tray icon window")?;
            conn.change_property8(
                PropMode::REPLACE,
                icon,
                AtomEnum::WM_NAME,
                AtomEnum::STRING,
                b"HRRR tray",
            )?
            .check()
            .context("name tray icon window")?;
            conn.change_property8(
                PropMode::REPLACE,
                icon,
                AtomEnum::WM_CLASS,
                AtomEnum::STRING,
                b"hrrr-tray\0hrrr-tray\0",
            )?
            .check()
            .context("classify tray icon window")?;
            conn.change_property32(
                PropMode::REPLACE,
                icon,
                atoms.xembed_info,
                atoms.xembed_info,
                &[0, XEMBED_MAPPED],
            )?
            .check()
            .context("declare XEmbed mapping")?;

            let menu = conn.generate_id().context("allocate tray menu window")?;
            let menu_background = alloc_color(&conn, colormap, [0x1d, 0x1f, 0x21])?;
            conn.create_window(
                depth,
                menu,
                root,
                0,
                0,
                MENU_WIDTH,
                MENU_HEIGHT,
                MENU_BORDER,
                WindowClass::INPUT_OUTPUT,
                visual,
                &CreateWindowAux::new()
                    .override_redirect(1)
                    .save_under(1)
                    .background_pixel(menu_background)
                    .border_pixel(alloc_color(&conn, colormap, [0xc5, 0xa8, 0x6a])?)
                    .event_mask(
                        EventMask::EXPOSURE | EventMask::BUTTON_PRESS | EventMask::KEY_PRESS,
                    ),
            )?
            .check()
            .context("create tray menu window")?;
            conn.change_property8(
                PropMode::REPLACE,
                menu,
                AtomEnum::WM_NAME,
                AtomEnum::STRING,
                b"HRRR tray menu",
            )?
            .check()
            .context("name tray menu window")?;

            let contour_gc = make_gc(
                &conn,
                icon,
                alloc_color(&conn, colormap, [0xe8, 0xdf, 0xc7])?,
                2,
                None,
            )?;
            let ember_gc = make_gc(
                &conn,
                icon,
                alloc_color(&conn, colormap, [0xd0, 0x77, 0x4b])?,
                1,
                None,
            )?;
            let font = conn.generate_id().context("allocate tray menu font")?;
            conn.open_font(font, b"fixed")?
                .check()
                .context("open tray menu font")?;
            let menu_gc = make_gc(
                &conn,
                menu,
                alloc_color(&conn, colormap, [0xe8, 0xdf, 0xc7])?,
                1,
                Some(font),
            )?;
            conn.close_font(font)?
                .check()
                .context("release tray menu font")?;

            let mut tray = Self {
                conn,
                root,
                root_width,
                root_height,
                icon,
                menu,
                contour_gc,
                ember_gc,
                menu_gc,
                atoms,
                owner: NONE,
                menu_live: false,
                available,
                emit,
            };
            tray.reconcile_owner()?;
            tray.conn.flush().context("flush tray creation")?;
            Ok(tray)
        }

        fn run(&mut self, alive: &AtomicBool, wake: &UnixStream) -> Result<()> {
            let mut owner_poll = Instant::now() + OWNER_POLL;
            while alive.load(Ordering::Acquire) {
                let timeout = owner_poll.saturating_duration_since(Instant::now());
                let (x_ready, wake_ready) = {
                    let mut descriptors = [
                        PollFd::new(self.conn.stream().as_fd(), PollFlags::POLLIN),
                        PollFd::new(wake.as_fd(), PollFlags::POLLIN),
                    ];
                    let timeout = PollTimeout::try_from(timeout).unwrap_or(PollTimeout::MAX);
                    let _ready = poll(&mut descriptors, timeout).context("wait for tray events")?;
                    (
                        descriptors[0]
                            .revents()
                            .is_some_and(|events| events.contains(PollFlags::POLLIN)),
                        descriptors[1]
                            .revents()
                            .is_some_and(|events| events.contains(PollFlags::POLLIN)),
                    )
                };
                if wake_ready {
                    break;
                }
                if x_ready {
                    while let Some(event) =
                        self.conn.poll_for_event().context("poll tray events")?
                    {
                        self.heed(event)?;
                    }
                }
                if Instant::now() >= owner_poll {
                    self.reconcile_owner()?;
                    owner_poll = Instant::now() + OWNER_POLL;
                }
            }
            Ok(())
        }

        fn reconcile_owner(&mut self) -> Result<()> {
            let owner = self
                .conn
                .get_selection_owner(self.atoms.selection)?
                .reply()
                .context("query X11 tray owner")?
                .owner;
            if owner != self.owner {
                self.owner = owner;
                self.available.store(owner != NONE, Ordering::Release);
                if owner != NONE {
                    let dock = ClientMessageEvent::new(
                        32,
                        owner,
                        self.atoms.opcode,
                        [CURRENT_TIME, DOCK_REQUEST, self.icon, 0, 0],
                    );
                    let _sent = self
                        .conn
                        .send_event(false, owner, EventMask::NO_EVENT, dock)?;
                    self.conn.flush().context("dock X11 tray icon")?;
                }
            }
            Ok(())
        }

        fn heed(&mut self, event: Event) -> Result<()> {
            match event {
                Event::Expose(event) if event.window == self.icon => self.paint_icon()?,
                Event::ConfigureNotify(event) if event.window == self.icon => self.paint_icon()?,
                Event::ButtonPress(event) if event.event == self.icon => {
                    self.click_icon(&event)?;
                }
                Event::Expose(event) if event.window == self.menu => self.paint_menu()?,
                Event::ButtonPress(event) if self.menu_live => self.click_menu(&event)?,
                Event::KeyPress(_) if self.menu_live => self.hide_menu()?,
                _ => {}
            }
            Ok(())
        }

        fn click_icon(&mut self, event: &ButtonPressEvent) -> Result<()> {
            match event.detail {
                1 => (self.emit)(Signal::Reveal),
                3 => self.show_menu(event.root_x, event.root_y)?,
                _ => {}
            }
            Ok(())
        }

        fn show_menu(&mut self, root_x: i16, root_y: i16) -> Result<()> {
            let point = [i32::from(root_x), i32::from(root_y)];
            let monitor = self.monitor_at(point);
            let anchor = self.icon_rect().unwrap_or(DesktopRect {
                x: point[0] - i32::from(ICON_SIZE) / 2,
                y: point[1] - i32::from(ICON_SIZE) / 2,
                width: i32::from(ICON_SIZE),
                height: i32::from(ICON_SIZE),
            });
            let [x, y] = popup_origin(
                monitor,
                anchor,
                [
                    i32::from(MENU_WIDTH + 2 * MENU_BORDER),
                    i32::from(MENU_HEIGHT + 2 * MENU_BORDER),
                ],
            );
            let _configured = self.conn.configure_window(
                self.menu,
                &x11rb::protocol::xproto::ConfigureWindowAux::new()
                    .x(x)
                    .y(y)
                    .stack_mode(x11rb::protocol::xproto::StackMode::ABOVE),
            )?;
            let _mapped = self.conn.map_window(self.menu)?;
            let _focused = self.conn.set_input_focus(
                x11rb::protocol::xproto::InputFocus::POINTER_ROOT,
                self.menu,
                CURRENT_TIME,
            )?;
            let _grab = self
                .conn
                .grab_pointer(
                    false,
                    self.menu,
                    EventMask::BUTTON_PRESS,
                    GrabMode::ASYNC,
                    GrabMode::ASYNC,
                    NONE,
                    NONE,
                    CURRENT_TIME,
                )?
                .reply();
            self.menu_live = true;
            self.conn.flush().context("show tray menu")?;
            Ok(())
        }

        fn icon_rect(&self) -> Option<DesktopRect> {
            let geometry = self.conn.get_geometry(self.icon).ok()?.reply().ok()?;
            let origin = self
                .conn
                .translate_coordinates(self.icon, self.root, 0, 0)
                .ok()?
                .reply()
                .ok()?;
            Some(DesktopRect {
                x: i32::from(origin.dst_x),
                y: i32::from(origin.dst_y),
                width: i32::from(geometry.width),
                height: i32::from(geometry.height),
            })
        }

        fn monitor_at(&self, point: [i32; 2]) -> DesktopRect {
            let root = DesktopRect {
                x: 0,
                y: 0,
                width: i32::from(self.root_width),
                height: i32::from(self.root_height),
            };
            let monitors = self
                .conn
                .randr_get_monitors(self.root, true)
                .ok()
                .and_then(|cookie| cookie.reply().ok())
                .map(|reply| {
                    reply
                        .monitors
                        .into_iter()
                        .map(|monitor| DesktopRect {
                            x: i32::from(monitor.x),
                            y: i32::from(monitor.y),
                            width: i32::from(monitor.width),
                            height: i32::from(monitor.height),
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            nearest_monitor(&monitors, point).unwrap_or(root)
        }

        fn click_menu(&mut self, event: &ButtonPressEvent) -> Result<()> {
            let inside = event.event_x >= 0
                && event.event_y >= 0
                && event.event_x < MENU_WIDTH as i16
                && event.event_y < MENU_HEIGHT as i16;
            self.hide_menu()?;
            if event.detail == 1 && inside {
                (self.emit)(Signal::Quit);
            }
            Ok(())
        }

        fn hide_menu(&mut self) -> Result<()> {
            let _ungrabbed = self.conn.ungrab_pointer(CURRENT_TIME)?;
            let _unmapped = self.conn.unmap_window(self.menu)?;
            self.menu_live = false;
            self.conn.flush().context("hide tray menu")?;
            Ok(())
        }

        fn paint_icon(&self) -> Result<()> {
            let geometry = self.conn.get_geometry(self.icon)?.reply()?;
            let _cleared = self.conn.clear_area(false, self.icon, 0, 0, 0, 0)?;
            let sx = f32::from(geometry.width) / f32::from(ICON_SIZE);
            let sy = f32::from(geometry.height) / f32::from(ICON_SIZE);
            let point = |x: i16, y: i16| Point {
                x: (f32::from(x) * sx).round() as i16,
                y: (f32::from(y) * sy).round() as i16,
            };
            for contour in [
                [(2, 7), (6, 5), (11, 6), (15, 9), (21, 7)],
                [(2, 12), (7, 10), (12, 12), (17, 13), (22, 11)],
                [(3, 17), (8, 15), (13, 17), (18, 18), (21, 17)],
            ] {
                let contour = contour.map(|(x, y)| point(x, y));
                let _painted =
                    self.conn
                        .poly_line(CoordMode::ORIGIN, self.icon, self.contour_gc, &contour)?;
            }
            let ember = XArc {
                x: point(13, 3).x,
                y: point(13, 3).y,
                width: (6.0 * sx).round().max(1.0) as u16,
                height: (6.0 * sy).round().max(1.0) as u16,
                angle1: 0,
                angle2: 360 * 64,
            };
            let _painted = self
                .conn
                .poly_fill_arc(self.icon, self.ember_gc, &[ember])?;
            self.conn.flush().context("paint tray icon")?;
            Ok(())
        }

        fn paint_menu(&self) -> Result<()> {
            let _cleared = self.conn.clear_area(false, self.menu, 0, 0, 0, 0)?;
            let _painted = self
                .conn
                .image_text8(self.menu, self.menu_gc, 13, 20, b"Quit HRRR")?;
            self.conn.flush().context("paint tray menu")?;
            Ok(())
        }
    }

    impl Drop for X11Tray {
        fn drop(&mut self) {
            self.available.store(false, Ordering::Release);
            let _destroyed = self.conn.destroy_window(self.menu);
            let _destroyed = self.conn.destroy_window(self.icon);
            let _freed = self.conn.free_gc(self.menu_gc);
            let _freed = self.conn.free_gc(self.ember_gc);
            let _freed = self.conn.free_gc(self.contour_gc);
            let _flushed = self.conn.flush();
        }
    }

    fn intern(conn: &RustConnection, name: &str) -> Result<Atom> {
        Ok(conn
            .intern_atom(false, name.as_bytes())?
            .reply()
            .with_context(|| format!("intern X11 atom `{name}`"))?
            .atom)
    }

    fn alloc_color(conn: &RustConnection, colormap: u32, [r, g, b]: [u8; 3]) -> Result<u32> {
        Ok(conn
            .alloc_color(
                colormap,
                u16::from(r) * 257,
                u16::from(g) * 257,
                u16::from(b) * 257,
            )?
            .reply()
            .context("allocate X11 tray color")?
            .pixel)
    }

    fn make_gc(
        conn: &RustConnection,
        drawable: Window,
        foreground: u32,
        width: u32,
        font: Option<u32>,
    ) -> Result<Gcontext> {
        let gc = conn
            .generate_id()
            .context("allocate tray graphics context")?;
        let mut attributes = CreateGCAux::new()
            .foreground(foreground)
            .line_width(width)
            .line_style(LineStyle::SOLID)
            .cap_style(CapStyle::ROUND)
            .join_style(JoinStyle::ROUND)
            .graphics_exposures(0);
        if let Some(font) = font {
            attributes = attributes.font(font);
        }
        conn.create_gc(gc, drawable, &attributes)?
            .check()
            .context("create tray graphics context")?;
        Ok(gc)
    }

    pub struct Tray {
        alive: Arc<AtomicBool>,
        available: Arc<AtomicBool>,
        wake: UnixStream,
        thread: Option<JoinHandle<()>>,
    }

    impl Tray {
        pub fn raise<F>(emit: F) -> Result<Self>
        where
            F: Fn(Signal) + Send + Sync + 'static,
        {
            let alive = Arc::new(AtomicBool::new(true));
            let available = Arc::new(AtomicBool::new(false));
            let mut tray = X11Tray::forge(available.clone(), Arc::new(emit))?;
            let (wake, thread_wake) = UnixStream::pair().context("forge tray wake pipe")?;
            let thread_alive = alive.clone();
            let thread = thread::Builder::new()
                .name("hrrr-xembed-tray".to_owned())
                .spawn(move || {
                    if let Err(error) = tray.run(&thread_alive, &thread_wake) {
                        eprintln!("X11 tray failed: {error:#}");
                    }
                })
                .context("spawn X11 tray")?;
            Ok(Self {
                alive,
                available,
                wake,
                thread: Some(thread),
            })
        }

        pub fn available(&self) -> bool {
            self.available.load(Ordering::Acquire)
        }
    }

    impl Drop for Tray {
        fn drop(&mut self) {
            self.alive.store(false, Ordering::Release);
            let _woken = self.wake.write_all(&[0]);
            if let Some(thread) = self.thread.take() {
                let _joined = thread.join();
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        const LIVE_TOPOLOGY: [DesktopRect; 3] = [
            DesktopRect {
                x: 0,
                y: 0,
                width: 1080,
                height: 1920,
            },
            DesktopRect {
                x: 1080,
                y: 300,
                width: 1920,
                height: 1080,
            },
            DesktopRect {
                x: 3000,
                y: 0,
                width: 1080,
                height: 1920,
            },
        ];

        #[test]
        fn tray_menu_is_imprisoned_on_the_icons_monitor() {
            let middle = nearest_monitor(&LIVE_TOPOLOGY, [2988, 1368]);
            assert_eq!(middle, Some(LIVE_TOPOLOGY[1]));
            let origin = popup_origin(
                middle.unwrap_or(LIVE_TOPOLOGY[0]),
                DesktopRect {
                    x: 2976,
                    y: 1356,
                    width: 24,
                    height: 24,
                },
                [106, 32],
            );
            assert_eq!(origin, [2894, 1320]);
        }

        #[test]
        fn tray_menu_falls_below_a_top_edge_icon() {
            assert_eq!(
                popup_origin(
                    LIVE_TOPOLOGY[1],
                    DesktopRect {
                        x: 2976,
                        y: 300,
                        width: 24,
                        height: 24,
                    },
                    [106, 32],
                ),
                [2894, 328]
            );
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
mod platform {
    use super::Signal;
    use anyhow::{Context as _, Result};
    use std::sync::Arc;
    use tray_icon::{
        Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
        menu::{Menu, MenuEvent, MenuItem},
    };

    pub struct Tray {
        _icon: TrayIcon,
        _menu: Menu,
        _open: MenuItem,
        _quit: MenuItem,
    }

    impl Tray {
        pub fn raise<F>(emit: F) -> Result<Self>
        where
            F: Fn(Signal) + Send + Sync + 'static,
        {
            let emit: Arc<dyn Fn(Signal) + Send + Sync> = Arc::new(emit);
            let menu = Menu::new();
            let open = MenuItem::new("Open HRRR", true, None);
            let quit = MenuItem::new("Quit", true, None);
            menu.append_items(&[&open, &quit])
                .context("build tray menu")?;
            let icon = TrayIconBuilder::new()
                .with_tooltip("HRRR forecast fields")
                .with_icon(icon()?)
                .with_icon_as_template(cfg!(target_os = "macos"))
                .with_menu(Box::new(menu.clone()))
                .with_menu_on_left_click(false)
                .build()
                .context("raise tray icon")?;

            let open_id = open.id().clone();
            let quit_id = quit.id().clone();
            let menu_emit = emit.clone();
            MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
                if event.id == open_id {
                    menu_emit(Signal::Reveal);
                } else if event.id == quit_id {
                    menu_emit(Signal::Quit);
                }
            }));
            TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
                if matches!(
                    event,
                    TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    }
                ) {
                    emit(Signal::Reveal);
                }
            }));
            Ok(Self {
                _icon: icon,
                _menu: menu,
                _open: open,
                _quit: quit,
            })
        }

        pub const fn available(&self) -> bool {
            true
        }
    }

    fn icon() -> Result<Icon> {
        const SIDE: u32 = 32;
        let mut rgba = vec![0_u8; (SIDE * SIDE * 4) as usize];
        for y in 0..SIDE {
            for x in 0..SIDE {
                let wave = [8_i32, 15, 22].into_iter().any(|axis| {
                    let bend = ((x as f32 * 0.55).sin() * 2.0).round() as i32;
                    (y as i32 - axis - bend).abs() <= 1 && (3..29).contains(&x)
                });
                let ember = (x as i32 - 22).pow(2) + (y as i32 - 7).pow(2) <= 13;
                if wave || ember {
                    let offset = ((y * SIDE + x) * 4) as usize;
                    rgba[offset..offset + 4].copy_from_slice(&[255, 255, 255, 255]);
                }
            }
        }
        Icon::from_rgba(rgba, SIDE, SIDE).context("forge tray icon")
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
mod platform {
    use super::Signal;
    use anyhow::{Result, bail};

    pub struct Tray;

    impl Tray {
        pub fn raise<F>(_emit: F) -> Result<Self>
        where
            F: Fn(Signal) + Send + Sync + 'static,
        {
            bail!("system tray is unsupported on this platform")
        }

        pub const fn available(&self) -> bool {
            false
        }
    }
}

pub use platform::Tray;
