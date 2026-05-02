#[cfg(target_os = "linux")]
fn main() {
    use ashta_kernel::diagnostics;

    match diagnostics::run() {
        Ok(report) => print!("{}", report.display()),
        Err(e) => {
            eprintln!("hft-check error: {e}");
            std::process::exit(1);
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("hft-check requires Linux");
    std::process::exit(1);
}
