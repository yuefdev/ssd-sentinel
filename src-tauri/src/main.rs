#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    #[cfg(target_os = "linux")]
    if args.first().map(String::as_str) == Some("--privileged-helper") {
        std::process::exit(ssd_sentinel_core::linux_priv::run_helper_stdio());
    }

    if matches!(args.first().map(String::as_str), Some("--version" | "-V")) {
        println!("ssd-sentinel {}", env!("CARGO_PKG_VERSION"));
        return;
    }

    if matches!(args.first().map(String::as_str), Some("--help" | "-h")) {
        ssd_sentinel_core::cli::print_help();
        return;
    }

    if args.first().map(String::as_str) == Some("--cli") {
        std::process::exit(ssd_sentinel_core::cli::run(&args[1..]));
    }

    #[cfg(all(target_os = "linux", feature = "native-gui"))]
    {
        std::process::exit(ssd_sentinel_core::native_gui::run());
    }

    #[cfg(all(
        feature = "desktop",
        not(all(target_os = "linux", feature = "native-gui"))
    ))]
    {
        ssd_sentinel_core::desktop::run();
    }

    #[cfg(all(
        not(feature = "desktop"),
        not(all(target_os = "linux", feature = "native-gui"))
    ))]
    {
        ssd_sentinel_core::cli::print_help();
        std::process::exit(2);
    }
}
