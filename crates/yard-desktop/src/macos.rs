use std::{
    env, fmt,
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
    ipc::CapabilityBuilder,
    menu::{MenuBuilder, MenuItem, SubmenuBuilder},
    webview::PageLoadEvent,
};
use yard_desktop::{
    AddMachineRequest, ExitDecision, ReadyService, ReconnectMachineRequest, ServiceAction,
    ServiceMode, ServiceStatus, add_machine_arguments, can_stop_service,
    desktop_navigation_allowed, exit_decision, fresh_service_environment,
    reconnect_machine_arguments, resolve_sidecar, run_bounded_command, service_action,
    service_origin_and_pattern,
};

use crate::terminal_handoff;

const MAIN_WINDOW: &str = "main";
const MENU_SHOW: &str = "show";
const MENU_STOP: &str = "stop-service";
const SERVICE_OVERRIDE: &str = "YARD_DESKTOP_SERVICE_BIN";
const HERDR_OVERRIDE: &str = "YARD_DESKTOP_HERDR_BIN";
const EXISTING_SERVICE_NOTICE: &str =
    "Yard — Existing service; Stop Yard Service, then Show Yard to adopt bundled Herdr";
const STATUS_TIMEOUT: Duration = Duration::from_secs(50);
const START_TIMEOUT: Duration = Duration::from_secs(80);
const STOP_TIMEOUT: Duration = Duration::from_secs(65);
const TERMINAL_ERROR: &str = "Could not launch Terminal. Try again or use the browser command.";
const ALLOW_MACHINE_ADD: &str = "allow-launch-machine-add";
const ALLOW_MACHINE_RECONNECT: &str = "allow-launch-machine-reconnect";

#[derive(serde::Serialize)]
struct LaunchReceipt {
    launched: bool,
}

#[tauri::command]
fn launch_machine_add(
    request: AddMachineRequest,
    lifecycle: tauri::State<'_, Arc<ServiceLifecycle>>,
) -> Result<LaunchReceipt, String> {
    let request = request.validate().map_err(|_| TERMINAL_ERROR.to_owned())?;
    terminal_handoff::launch(&lifecycle.herdr_binary, &add_machine_arguments(&request))
        .map_err(|_| TERMINAL_ERROR.to_owned())?;
    Ok(LaunchReceipt { launched: true })
}

#[tauri::command]
fn launch_machine_reconnect(
    request: ReconnectMachineRequest,
    lifecycle: tauri::State<'_, Arc<ServiceLifecycle>>,
) -> Result<LaunchReceipt, String> {
    let machine_id = request.validate().map_err(|_| TERMINAL_ERROR.to_owned())?;
    terminal_handoff::launch(
        &lifecycle.herdr_binary,
        &reconnect_machine_arguments(machine_id),
    )
    .map_err(|_| TERMINAL_ERROR.to_owned())?;
    Ok(LaunchReceipt { launched: true })
}

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
    let current =
        env::current_exe().map_err(|error| format!("could not locate Yard.app: {error}"))?;
    let lifecycle = Arc::new(ServiceLifecycle::new(
        resolve_service_binary(&current)?,
        resolve_herdr_binary(&current)?,
    ));
    let setup_lifecycle = Arc::clone(&lifecycle);
    let menu_lifecycle = Arc::clone(&lifecycle);

    let last_window_destroyed = Arc::new(AtomicBool::new(false));
    let run_last_window_destroyed = Arc::clone(&last_window_destroyed);

    let app = tauri::Builder::default()
        .manage(Arc::clone(&lifecycle))
        .invoke_handler(tauri::generate_handler![
            launch_machine_add,
            launch_machine_reconnect
        ])
        .setup(move |app| {
            let stop_service = install_menu(app)?;
            app.manage(DesktopMenu { stop_service });
            let window = loading_window(app.handle(), Arc::clone(&lifecycle))?;
            let lifecycle = Arc::clone(&setup_lifecycle);
            #[cfg(target_os = "macos")]
            app.handle()
                .set_activation_policy(tauri::ActivationPolicy::Regular)?;
            thread::spawn(move || {
                let result = lifecycle.ensure_ready();
                apply_stop_capability(&window, &lifecycle);
                match result {
                    Ok(service) => navigate_to_service(&window, &service),
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

fn loading_window(
    app: &AppHandle,
    lifecycle: Arc<ServiceLifecycle>,
) -> tauri::Result<WebviewWindow> {
    let page_lifecycle = Arc::clone(&lifecycle);
    WebviewWindowBuilder::new(app, MAIN_WINDOW, WebviewUrl::App("loading.html".into()))
        .on_page_load(move |window, payload| {
            if payload.event() == PageLoadEvent::Finished
                && payload.url().scheme() == "http"
                && page_lifecycle.attached_to_existing()
            {
                let _ = window.set_title(EXISTING_SERVICE_NOTICE);
            }
        })
        .on_navigation(move |url| navigation_allowed(url, lifecycle.service_origin()))
        .title("Yard — Connecting")
        .inner_size(1280.0, 820.0)
        .min_inner_size(800.0, 600.0)
        .build()
}

fn navigation_allowed(url: &tauri::Url, service_origin: Option<String>) -> bool {
    desktop_navigation_allowed(url.as_str(), service_origin.as_deref())
}

fn install_machine_capability(window: &WebviewWindow, service_url: &str) -> Result<(), String> {
    let (origin, pattern) = service_origin_and_pattern(service_url)?;
    window
        .app_handle()
        .add_capability(
            CapabilityBuilder::new(format!(
                "yard-machine-terminal-{}",
                origin.replace([':', '/', '[', ']'], "-")
            ))
            .local(false)
            .window(MAIN_WINDOW)
            .remote(pattern)
            .permission(ALLOW_MACHINE_ADD)
            .permission(ALLOW_MACHINE_RECONNECT),
        )
        .map_err(|_| "could not authorize Yard Terminal handoff".to_owned())?;
    Ok(())
}

fn navigate_to_service(window: &WebviewWindow, service: &DesktopReadyService) {
    if let Err(error) = install_machine_capability(window, &service.service.url) {
        show_failure(window, &error);
        return;
    }
    service.lifecycle.set_service_origin(&service.service.url);
    match service.service.url.parse() {
        Ok(url) => {
            if service.attached_to_existing {
                let _ = window.set_title(EXISTING_SERVICE_NOTICE);
            }
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
        match loading_window(app, Arc::clone(&lifecycle)) {
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
                let already_connected = window
                    .url()
                    .is_ok_and(|url| url.as_str() == service.service.url);
                if service.attached_to_existing {
                    let _ = window.set_title(EXISTING_SERVICE_NOTICE);
                }
                if !already_connected {
                    navigate_to_service(&window, &service);
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
    herdr_binary: PathBuf,
    attached_to_existing: AtomicBool,
    operation: Mutex<()>,
    service_origin: Mutex<Option<String>>,
    stop_available: AtomicBool,
}

struct DesktopReadyService {
    service: ReadyService,
    attached_to_existing: bool,
    lifecycle: Arc<ServiceLifecycle>,
}

impl ServiceLifecycle {
    fn new(binary: PathBuf, herdr_binary: PathBuf) -> Self {
        Self {
            binary,
            herdr_binary,
            attached_to_existing: AtomicBool::new(false),
            operation: Mutex::new(()),
            service_origin: Mutex::new(None),
            stop_available: AtomicBool::new(false),
        }
    }

    fn ensure_ready(self: &Arc<Self>) -> Result<DesktopReadyService, String> {
        let operation = self
            .operation
            .try_lock()
            .map_err(|_| "a Yard desktop lifecycle operation is already in progress".to_owned())?;
        let initial = self.command(
            ["status", "--json"],
            std::iter::empty::<(&str, &str)>(),
            STATUS_TIMEOUT,
        )?;
        self.stop_available.store(false, Ordering::Release);
        self.attached_to_existing.store(false, Ordering::Release);
        let status = parse_status(&initial)?;
        let result = match service_action(&status)? {
            ServiceAction::Attach => {
                self.stop_available.store(true, Ordering::Release);
                self.attached_to_existing.store(true, Ordering::Release);
                status.ready().map(|service| DesktopReadyService {
                    service,
                    attached_to_existing: true,
                    lifecycle: Arc::clone(self),
                })
            }
            ServiceAction::Start => {
                let started = self.command(
                    ["start", "--no-open"],
                    fresh_service_environment(&self.herdr_binary),
                    START_TIMEOUT,
                )?;
                require_success("start Yard", &started)?;
                let status = self.command(
                    ["status", "--json"],
                    std::iter::empty::<(&str, &str)>(),
                    STATUS_TIMEOUT,
                )?;
                let status = parse_status(&status)?;
                self.stop_available
                    .store(can_stop_service(&status), Ordering::Release);
                status.ready().map(|service| DesktopReadyService {
                    service,
                    attached_to_existing: false,
                    lifecycle: Arc::clone(self),
                })
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
        let status = self.command(
            ["status", "--json"],
            std::iter::empty::<(&str, &str)>(),
            STATUS_TIMEOUT,
        )?;
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
        let output = self.command(["stop"], std::iter::empty::<(&str, &str)>(), STOP_TIMEOUT)?;
        require_success("stop Yard", &output)?;
        self.stop_available.store(false, Ordering::Release);
        if let Ok(mut origin) = self.service_origin.lock() {
            *origin = None;
        }
        Ok(())
    }

    fn stop_available(&self) -> bool {
        self.stop_available.load(Ordering::Acquire)
    }

    fn attached_to_existing(&self) -> bool {
        self.attached_to_existing.load(Ordering::Acquire)
    }

    fn service_origin(&self) -> Option<String> {
        self.service_origin.lock().ok()?.clone()
    }

    fn set_service_origin(&self, service_url: &str) {
        if let Ok((origin, _)) = service_origin_and_pattern(service_url)
            && let Ok(mut current) = self.service_origin.lock()
        {
            *current = Some(origin);
        }
    }

    fn command<const N: usize, E, K, V>(
        &self,
        args: [&str; N],
        environment: E,
        deadline: Duration,
    ) -> Result<yard_desktop::CommandOutput, String>
    where
        E: IntoIterator<Item = (K, V)>,
        K: AsRef<std::ffi::OsStr>,
        V: AsRef<std::ffi::OsStr>,
    {
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

fn resolve_service_binary(current: &std::path::Path) -> Result<PathBuf, String> {
    resolve_sidecar(
        current,
        "yard",
        env::var_os(SERVICE_OVERRIDE).as_deref(),
        SERVICE_OVERRIDE,
    )
}

fn resolve_herdr_binary(current: &std::path::Path) -> Result<PathBuf, String> {
    resolve_sidecar(
        current,
        "herdr",
        env::var_os(HERDR_OVERRIDE).as_deref(),
        HERDR_OVERRIDE,
    )
}

impl fmt::Debug for ServiceLifecycle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ServiceLifecycle")
            .field("binary", &self.binary)
            .field("herdr_binary", &self.herdr_binary)
            .finish_non_exhaustive()
    }
}
