#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    ssd_sentinel_core::desktop::run();
}
