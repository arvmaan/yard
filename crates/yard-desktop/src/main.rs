#[cfg(target_os = "macos")]
mod macos;
#[cfg(any(target_os = "macos", test))]
mod terminal_handoff;

#[cfg(target_os = "macos")]
fn main() {
    macos::main();
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("yard-desktop is a macOS proof of concept");
    std::process::exit(2);
}
