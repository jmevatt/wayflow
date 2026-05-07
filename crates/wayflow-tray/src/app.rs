//! Tray app entry point. Owns a `tao` event loop, a tray-icon, and the
//! subprocess supervisor. Intentionally minimal: no GUI window, no rendering.
//! Settings are edited via $EDITOR on the existing toml config files.

use anyhow::Result;
use std::path::PathBuf;
use std::time::Duration;

use tao::event::Event;
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tray_icon::{
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    TrayIcon, TrayIconBuilder, TrayIconEvent,
};
use wayflow_core::config::{ClientConfig, Config};

use crate::icon;
use crate::supervisor::{Mode, State, Supervisor};

/// Run the tray. Blocks until the user picks Quit.
pub fn run() -> Result<()> {
    // tao's event loop. On Linux this initializes gtk for us; on Mac it
    // sets up NSApplication; on Windows it owns the message pump. tray-icon
    // hooks into all three.
    let event_loop = EventLoopBuilder::new().build();

    // The tray icon, menu, and a wrapper we mutate from the loop.
    let mut state = TrayState::new()?;

    // tray-icon channels. They're MPSC receivers; pull on the event loop tick.
    let menu_rx = MenuEvent::receiver();
    let _tray_rx = TrayIconEvent::receiver(); // we don't currently react to clicks-on-icon

    event_loop.run(move |event, _, control_flow| {
        // Wake up at least every 500ms so we can poll the supervised child.
        *control_flow =
            ControlFlow::WaitUntil(std::time::Instant::now() + Duration::from_millis(500));

        // Service menu clicks (the only event source we care about right now).
        while let Ok(ev) = menu_rx.try_recv() {
            state.handle_menu(&ev.id);
            if state.should_exit {
                *control_flow = ControlFlow::Exit;
                return;
            }
        }

        // Tick: reap exited child + refresh menu/icon.
        if let Event::NewEvents(_) = event {
            state.supervisor.poll();
            let s = state.supervisor.state();
            state.refresh_for_state(&s);
        }
    });
}

struct TrayState {
    supervisor: Supervisor,
    tray: TrayIcon,
    items: MenuItems,
    last_state: State,
    config_path: PathBuf,
    client_config_path: PathBuf,
    should_exit: bool,
}

struct MenuItems {
    status: MenuItem,
    start_server: MenuItem,
    start_client: MenuItem,
    stop: MenuItem,
    edit_server: MenuItem,
    edit_client: MenuItem,
    open_log: MenuItem,
    open_config: MenuItem,
    quit: MenuItem,
}

impl TrayState {
    fn new() -> Result<Self> {
        let supervisor = Supervisor::new();

        let menu = Menu::new();
        let status = MenuItem::new("status: stopped", false, None);
        let start_server = MenuItem::new("Start server", true, None);
        let start_client = MenuItem::new("Start client", true, None);
        let stop = MenuItem::new("Stop", false, None);
        let edit_server = MenuItem::new("Edit server config", true, None);
        let edit_client = MenuItem::new("Edit client config", true, None);
        let open_log = MenuItem::new("Open log dir", true, None);
        let open_config = MenuItem::new("Open config dir", true, None);
        let quit = MenuItem::new("Quit wayflow", true, None);

        menu.append(&status)?;
        menu.append(&PredefinedMenuItem::separator())?;
        menu.append(&start_server)?;
        menu.append(&start_client)?;
        menu.append(&stop)?;
        menu.append(&PredefinedMenuItem::separator())?;
        menu.append(&edit_server)?;
        menu.append(&edit_client)?;
        menu.append(&open_log)?;
        menu.append(&open_config)?;
        menu.append(&PredefinedMenuItem::separator())?;
        menu.append(&quit)?;

        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("Wayflow (stopped)")
            .with_icon(icon::idle())
            .build()?;

        Ok(Self {
            supervisor,
            tray,
            items: MenuItems {
                status,
                start_server,
                start_client,
                stop,
                edit_server,
                edit_client,
                open_log,
                open_config,
                quit,
            },
            last_state: State::Stopped,
            config_path: Config::default_path(),
            client_config_path: ClientConfig::default_path(),
            should_exit: false,
        })
    }

    fn handle_menu(&mut self, id: &tray_icon::menu::MenuId) {
        if id == self.items.start_server.id() {
            if let Err(e) = self.supervisor.start(Mode::Server) {
                tracing::error!("start server: {e:#}");
            }
        } else if id == self.items.start_client.id() {
            if let Err(e) = self.supervisor.start(Mode::Client) {
                tracing::error!("start client: {e:#}");
            }
        } else if id == self.items.stop.id() {
            self.supervisor.stop();
        } else if id == self.items.edit_server.id() {
            ensure_parent(&self.config_path);
            open_in_editor(&self.config_path);
        } else if id == self.items.edit_client.id() {
            ensure_parent(&self.client_config_path);
            open_in_editor(&self.client_config_path);
        } else if id == self.items.open_log.id() {
            let _ = open::that_detached(log_dir());
        } else if id == self.items.open_config.id() {
            if let Some(parent) = self.config_path.parent() {
                let _ = std::fs::create_dir_all(parent);
                let _ = open::that_detached(parent);
            }
        } else if id == self.items.quit.id() {
            self.supervisor.stop();
            self.should_exit = true;
        }
    }

    fn refresh_for_state(&mut self, state: &State) {
        if *state == self.last_state {
            return;
        }
        let label = match state {
            State::Stopped => "status: stopped".to_string(),
            State::Running { mode, pid } => {
                format!("status: running {} (pid {})", mode.label(), pid)
            }
            State::Crashed { mode, code } => match code {
                Some(c) => format!("status: {} crashed (exit {})", mode.label(), c),
                None => format!("status: {} crashed", mode.label()),
            },
        };
        self.items.status.set_text(&label);

        let running = state.is_running();
        self.items.start_server.set_enabled(!running);
        self.items.start_client.set_enabled(!running);
        self.items.stop.set_enabled(running);

        let new_icon = match state {
            State::Stopped => icon::idle(),
            State::Running { .. } => icon::running(),
            State::Crashed { .. } => icon::error(),
        };
        let _ = self.tray.set_icon(Some(new_icon));
        let _ = self.tray.set_tooltip(Some(format!(
            "Wayflow ({})",
            label.trim_start_matches("status: ")
        )));

        self.last_state = state.clone();
    }
}

fn log_dir() -> PathBuf {
    dirs::cache_dir()
        .map(|p| p.join("wayflow"))
        .unwrap_or_else(|| PathBuf::from("/tmp/wayflow"))
}

fn ensure_parent(p: &std::path::Path) {
    if let Some(parent) = p.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if !p.exists() {
        // Touch an empty file so $EDITOR has something to open.
        let _ = std::fs::write(p, "");
    }
}

/// Best-effort: open the file in $EDITOR if set; otherwise fall back to the
/// platform association via the `open` crate.
fn open_in_editor(p: &std::path::Path) {
    if let Ok(editor) = std::env::var("EDITOR") {
        if !editor.is_empty() {
            // Run detached so the tray doesn't block. For terminal editors
            // ($EDITOR=vim) the user gets a flash; that's expected -- they
            // can set it to a graphical one (code, gedit, etc.) for a
            // smoother experience.
            let _ = std::process::Command::new(&editor).arg(p).spawn();
            return;
        }
    }
    let _ = open::that_detached(p);
}
