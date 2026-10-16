#[cfg(target_os = "linux")]
mod linux;

#[cfg(target_os = "linux")]
fn main() -> Result<(), linux::DemoError> {
    linux::run()
}

#[cfg(not(target_os = "linux"))]
fn main() -> std::process::ExitCode {
    eprintln!("GNative is only supported on Linux");
    std::process::ExitCode::FAILURE
}
