use std::{
    env,
    ffi::OsString,
    fmt,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use tauri::{
    AppHandle, Manager, RunEvent, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent,
    menu::{MenuBuilder, MenuItem, SubmenuBuilder},
};
use url::Host;
use yard_desktop::{
    ExitDecision, ReadyService, ServiceAction, ServiceMode, ServiceStatus, can_stop_service,
    exit_decision, run_bounded_command, service_action,
};

const MAIN_WINDOW: &str = "main";
const MENU_SHOW: &str = "show";
const MENU_STOP: &str = "stop-service";
const SERVICE_OVERRIDE: &str = "YARD_DESKTOP_SERVICE_BIN";
const STATUS_TIMEOUT: Duration = Duration::from_secs(50);
const START_TIMEOUT: Duration = Duration::from_secs(80);
const STOP_TIMEOUT: Duration = Duration::from_secs(65);

struct DesktopMenu {
    stop_service: MenuItem<tauri::Wry>,
}

pub(crate) fn main() {
    if let Err(error) = run() {
        eprintln!("yard-desktop: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let lifecycle = Arc::new(ServiceLifecycle::new(resolve_service_binary()?));
    let setup_lifecycle = Arc::clone(&lifecycle);
    let menu_lifecycle = Arc::clone(&lifecycle);

    let last_window_destroyed = Arc::new(AtomicBool::new(false));
    let run_last_window_destroyed = Arc::clone(&last_window_destroyed);

    let app = tauri::Builder::default()
        .setup(move |app| {
            let stop_service = install_menu(app)?;
            app.manage(DesktopMenu { stop_service });
            let window = loading_window(app.handle())?;
            let lifecycle = Arc::clone(&setup_lifecycle);
            #[cfg(target_os = "macos")]
            app.handle()
                .set_activation_policy(tauri::ActivationPolicy::Regular)?;
            thread::spawn(move || {
                let result = lifecycle.ensure_ready();
                apply_stop_capability(&window, &lifecycle);
                match result {
                    Ok(service) => navigate_to_service(&window, &service.url),
                    Err(error) => show_failure(&window, &error),
                }
            });
            Ok(())
        })
        .on_menu_event(move |app, event| match event.id().as_ref() {
            MENU_SHOW => show_or_create_window(app, Arc::clone(&menu_lifecycle)),
            MENU_STOP => stop_service(app, Arc::clone(&menu_lifecycle)),
            _ => {}
        })
        .build(tauri::generate_context!())?;

    app.run(move |_app, event| match event {
        RunEvent::WindowEvent {
            event: WindowEvent::Destroyed,
            ..
        } => last_window_destroyed.store(true, Ordering::Release),
        RunEvent::ExitRequested { api, code, .. }
            if exit_decision(
                run_last_window_destroyed.swap(false, Ordering::AcqRel),
                code,
            ) == ExitDecision::PreventLastWindowExit =>
        {
            // Wry emits this immediately after the last window is destroyed.
            // Native Cmd-Q/system quit is a separate request and remains unblocked.
            api.prevent_exit();
        }
        _ => {}
    });
    Ok(())
}

fn install_menu(app: &tauri::App) -> tauri::Result<MenuItem<tauri::Wry>> {
    let stop_service = MenuItem::with_id(app, MENU_STOP, "Stop Yard Service", false, None::<&str>)?;
    let yard = SubmenuBuilder::new(app, "Yard")
        .text(MENU_SHOW, "Show Yard")
        .separator()
        .item(&stop_service)
        .separator()
        .quit()
        .build()?;
    app.set_menu(MenuBuilder::new(app).item(&yard).build()?)?;
    Ok(stop_service)
}

fn loading_window(app: &AppHandle) -> tauri::Result<WebviewWindow> {
    WebviewWindowBuilder::new(app, MAIN_WINDOW, WebviewUrl::App("loading.html".into()))
        .on_navigation(|url| {
            url.scheme() == "tauri"
                || (url.scheme() == "http"
                    && url.host().is_some_and(|host| match host {
                        Host::Ipv4(address) => address.is_loopback(),
                        Host::Ipv6(address) => address.is_loopback(),
                        Host::Domain(_) => false,
                    }))
        })
        .title("Yard — Connecting")
        .inner_size(1280.0, 820.0)
        .min_inner_size(800.0, 600.0)
        .build()
}

fn navigate_to_service(window: &WebviewWindow, url: &str) {
    match url.parse() {
        Ok(url) => {
            if let Err(error) = window.navigate(url) {
                show_failure(window, &format!("could not load Yard: {error}"));
            }
        }
        Err(error) => show_failure(window, &format!("service returned an invalid URL: {error}")),
    }
}

fn show_failure(window: &WebviewWindow, message: &str) {
    let escaped =
        serde_json::to_string(message).unwrap_or_else(|_| "\"Yard failed to start\"".into());
    let _ = window.eval(&format!(
        "document.getElementById('message').textContent = {escaped}; document.body.dataset.state = 'error';"
    ));
    let _ = window.set_title("Yard — Service unavailable");
}

fn show_or_create_window(app: &AppHandle, lifecycle: Arc<ServiceLifecycle>) {
    let window = if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        let _ = window.show();
        let _ = window.set_focus();
        window
    } else {
        match loading_window(app) {
            Ok(window) => window,
            Err(error) => {
                eprintln!("yard-desktop: could not create window: {error}");
                return;
            }
        }
    };
    thread::spawn(move || {
        let result = lifecycle.ensure_ready();
        apply_stop_capability(&window, &lifecycle);
        match result {
            Ok(service) => {
                let already_connected = window.url().is_ok_and(|url| url.as_str() == service.url);
                if !already_connected {
                    navigate_to_service(&window, &service.url);
                }
            }
            Err(error) => show_failure(&window, &error),
        }
    });
}

fn apply_stop_capability(window: &WebviewWindow, lifecycle: &ServiceLifecycle) {
    let menu = window.app_handle().state::<DesktopMenu>();
    if let Err(error) = menu.stop_service.set_enabled(lifecycle.stop_available()) {
        eprintln!("yard-desktop: could not update Stop Yard Service: {error}");
    }
}

fn stop_service(app: &AppHandle, lifecycle: Arc<ServiceLifecycle>) {
    let window = app.get_webview_window(MAIN_WINDOW);
    thread::spawn(move || {
        let result = lifecycle.stop();
        if let Some(window) = &window {
            apply_stop_capability(window, &lifecycle);
        }
        match result {
            Ok(()) => {
                if let Some(window) = window {
                    let url = tauri::Url::parse("tauri://localhost/stopped.html")
                        .expect("static stopped URL is valid");
                    let _ = window.navigate(url);
                    let _ = window.set_title("Yard — Service stopped");
                }
            }
            Err(error) => {
                if let Some(window) = window {
                    show_failure(&window, &error);
                } else {
                    eprintln!("yard-desktop: {error}");
                }
            }
        }
    });
}

struct ServiceLifecycle {
    binary: PathBuf,
    operation: Mutex<()>,
    stop_available: AtomicBool,
}

impl ServiceLifecycle {
    fn new(binary: PathBuf) -> Self {
        Self {
            binary,
            operation: Mutex::new(()),
            stop_available: AtomicBool::new(false),
        }
    }

    fn ensure_ready(&self) -> Result<ReadyService, String> {
        let operation = self
            .operation
            .try_lock()
            .map_err(|_| "a Yard desktop lifecycle operation is already in progress".to_owned())?;
        let initial = self.command(["status", "--json"], [], STATUS_TIMEOUT)?;
        self.stop_available.store(false, Ordering::Release);
        let status = parse_status(&initial)?;
        let result = match service_action(&status)? {
            ServiceAction::Attach => {
                self.stop_available.store(true, Ordering::Release);
                status.ready()
            }
            ServiceAction::Start => {
                let started = self.command(
                    ["start", "--no-open"],
                    [("YARD_BIND", "127.0.0.1:0")],
                    START_TIMEOUT,
                )?;
                require_success("start Yard", &started)?;
                let status = self.command(["status", "--json"], [], STATUS_TIMEOUT)?;
                let status = parse_status(&status)?;
                self.stop_available
                    .store(can_stop_service(&status), Ordering::Release);
                status.ready()
            }
            ServiceAction::Wait => {
                self.stop_available.store(false, Ordering::Release);
                Err(
                    "yard service is stopping; retry Show Yard after it finishes the transition"
                        .to_owned(),
                )
            }
        };
        drop(operation);
        result
    }

    fn stop(&self) -> Result<(), String> {
        let _operation = self
            .operation
            .try_lock()
            .map_err(|_| "a Yard desktop lifecycle operation is already in progress".to_owned())?;
        let status = self.command(["status", "--json"], [], STATUS_TIMEOUT)?;
        self.stop_available.store(false, Ordering::Release);
        let status = parse_status(&status)?;
        self.stop_available
            .store(can_stop_service(&status), Ordering::Release);
        if status.mode == Some(ServiceMode::Foreground) {
            self.stop_available.store(false, Ordering::Release);
            return Err(
                "the active Yard service is foreground-owned and must be stopped in its owning terminal"
                    .to_owned(),
            );
        }
        let output = self.command(["stop"], [], STOP_TIMEOUT)?;
        require_success("stop Yard", &output)?;
        self.stop_available.store(false, Ordering::Release);
        Ok(())
    }

    fn stop_available(&self) -> bool {
        self.stop_available.load(Ordering::Acquire)
    }

    fn command<const N: usize, const E: usize>(
        &self,
        args: [&str; N],
        environment: [(&str, &str); E],
        deadline: Duration,
    ) -> Result<yard_desktop::CommandOutput, String> {
        run_bounded_command(&self.binary, args, environment, deadline)
    }
}

fn parse_status(output: &yard_desktop::CommandOutput) -> Result<ServiceStatus, String> {
    if !output.status.success() && output.status.code() != Some(1) {
        return Err(command_failure("inspect Yard", output));
    }
    if output.stdout_truncated {
        return Err("yard status returned an oversized response".to_owned());
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("yard status returned invalid JSON: {error}"))
}

fn require_success(operation: &str, output: &yard_desktop::CommandOutput) -> Result<(), String> {
    if output.status.success() {
        Ok(())
    } else {
        Err(command_failure(operation, output))
    }
}

fn command_failure(operation: &str, output: &yard_desktop::CommandOutput) -> String {
    let detail = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    let suffix = if output.stderr_truncated {
        " (diagnostic output truncated)"
    } else {
        ""
    };
    if detail.is_empty() {
        format!(
            "could not {operation}: process exited with {}",
            output.status
        )
    } else {
        format!("could not {operation}: {detail}{suffix}")
    }
}

fn resolve_service_binary() -> Result<PathBuf, String> {
    if let Some(path) = env::var_os(SERVICE_OVERRIDE) {
        return require_executable(PathBuf::from(path), SERVICE_OVERRIDE);
    }

    let current =
        env::current_exe().map_err(|error| format!("could not locate Yard.app: {error}"))?;
    if let Some(macos) = current.parent() {
        let bundled = macos.join("yard");
        if bundled.is_file() {
            return Ok(bundled);
        }
    }

    find_on_path(OsString::from("yard")).ok_or_else(|| {
        format!(
            "could not locate the bundled Yard service; set {SERVICE_OVERRIDE} to a built yard executable"
        )
    })
}

fn require_executable(path: PathBuf, variable: &str) -> Result<PathBuf, String> {
    if path.is_file() {
        Ok(path)
    } else {
        Err(format!(
            "{variable} does not name a file: {}",
            path.display()
        ))
    }
}

fn find_on_path(name: OsString) -> Option<PathBuf> {
    env::var_os("PATH").and_then(|path| {
        env::split_paths(&path)
            .map(|directory| directory.join(&name))
            .find(|candidate| candidate.is_file())
    })
}

impl fmt::Debug for ServiceLifecycle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ServiceLifecycle")
            .field("binary", &self.binary)
            .finish_non_exhaustive()
    }
}
