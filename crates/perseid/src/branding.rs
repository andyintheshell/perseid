//! Embedded brand assets. The source files live in the repository's
//! `branding/` directory; `crates/perseid/assets/` holds the packaged copies
//! that are compiled into the binary.

use egui::{ColorImage, Context, IconData, TextureHandle, TextureOptions};
#[cfg(any(target_os = "macos", test))]
use std::path::Path;

const ICON_PNG: &[u8] = include_bytes!("../assets/icon-128.png");
const LOCKUP_PNG: &[u8] = include_bytes!("../assets/logo-light.png");
const MARK_PNG: &[u8] = include_bytes!("../assets/mark.png");

/// Canonical repository URL used by the About window and help links.
pub const REPOSITORY_URL: &str = "https://github.com/andyintheshell/perseid";

/// Hosted documentation root for the repository's `docs/` directory.
pub const DOCS_URL: &str = "https://github.com/andyintheshell/perseid/blob/main/docs";

/// Window/task-bar icon, decoded from the 128 px app icon.
pub fn window_icon() -> Option<IconData> {
    let image = image::load_from_memory(ICON_PNG).ok()?.into_rgba8();
    let (width, height) = image.dimensions();
    Some(IconData {
        rgba: image.into_raw(),
        width,
        height,
    })
}

/// Whether `executable` is inside a macOS app bundle. Packaged macOS apps
/// should use the bundle's ICNS icon so the system can apply its native mask.
#[cfg(any(target_os = "macos", test))]
pub(crate) fn is_macos_app_bundle_executable(executable: &Path) -> bool {
    let Some(macos_dir) = executable.parent() else {
        return false;
    };
    let Some(contents_dir) = macos_dir.parent() else {
        return false;
    };
    let Some(bundle_dir) = contents_dir.parent() else {
        return false;
    };

    path_component_is(macos_dir, "MacOS")
        && path_component_is(contents_dir, "Contents")
        && bundle_dir
            .extension()
            .is_some_and(|extension| extension.to_string_lossy().eq_ignore_ascii_case("app"))
}

#[cfg(any(target_os = "macos", test))]
fn path_component_is(path: &Path, expected: &str) -> bool {
    path.file_name()
        .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case(expected))
}

/// Transparent mark used in the toolbar.
pub fn mark_texture(ctx: &Context) -> Option<TextureHandle> {
    load_texture(ctx, "perseid.mark", MARK_PNG)
}

/// Full logo lockup (mark, wordmark, and tagline) used in the About window.
pub fn lockup_texture(ctx: &Context) -> Option<TextureHandle> {
    load_texture(ctx, "perseid.lockup", LOCKUP_PNG)
}

fn load_texture(ctx: &Context, name: &str, png: &[u8]) -> Option<TextureHandle> {
    let image = match image::load_from_memory(png) {
        Ok(image) => image.into_rgba8(),
        Err(err) => {
            log::warn!("could not decode {name}: {err}");
            return None;
        }
    };
    let size = [image.width() as usize, image.height() as usize];
    let color = ColorImage::from_rgba_unmultiplied(size, image.as_raw());
    Some(ctx.load_texture(name, color, TextureOptions::LINEAR))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packaged_assets_decode() {
        assert!(image::load_from_memory(ICON_PNG).is_ok());
        assert!(image::load_from_memory(LOCKUP_PNG).is_ok());
        assert!(image::load_from_memory(MARK_PNG).is_ok());
    }

    #[test]
    fn recognizes_macos_app_bundle_executable_paths() {
        assert!(is_macos_app_bundle_executable(Path::new(
            "/Applications/Perseid.app/Contents/MacOS/perseid"
        )));
        assert!(is_macos_app_bundle_executable(Path::new(
            "/Applications/Perseid Beta.APP/Contents/MacOS/perseid"
        )));

        assert!(!is_macos_app_bundle_executable(Path::new(
            "/Users/test/perseid/target/release/perseid"
        )));
        assert!(!is_macos_app_bundle_executable(Path::new(
            "/opt/Contents/MacOS/perseid"
        )));
        assert!(!is_macos_app_bundle_executable(Path::new(
            "/Applications/Perseid.app/Contents/Resources/perseid"
        )));
    }
}
