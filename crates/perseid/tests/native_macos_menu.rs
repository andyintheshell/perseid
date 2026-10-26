//! AppKit requires the process main thread, so this test uses its own harness.
fn main() {
    #[cfg(target_os = "macos")]
    perseid::test_native_macos_menu();
}
