# Independent utility window checks

[Documentation index](README.md) · [Workbench](workbench.md#independent-utility-windows)

Run these checks on macOS, Windows, Linux/X11, and Linux/Wayland with a graphical
desktop; include two monitors and, where available, different display scales.
These are manual checks, not claims of completed platform validation. Headless
tests cover viewport identity, geometry serialization, size capture without
global positions, close/Esc routing, parent focus commands, and retained state.

1. Open Search via the toolbar and `Ctrl`/`Cmd`+`Shift`+`F`. Confirm there is one
   native Search window, that its query receives focus, and that invoking the
   shortcut again raises the existing window rather than creating another.
2. Populate the Comparer and compare two items. Keep Search and the result open
   together. Drag each by its OS title bar to the second monitor; resize each
   beyond the main window's bounds and continue interacting with all windows.
   Check title-bar dragging, text input, result selection, diff navigation,
   scrolling, and software/hardware rendering as applicable.
3. Press `Esc` in one child. Only that child should close; the main window should
   regain focus. Repeat with the native close button and with the other child.
   `Esc` in the main window must not close either native utility window.
4. Reopen Search and rerun the comparison. Confirm queries/options/results and
   Comparer inputs remain available, and that each window restores its own
   normal size/position. Maximize/minimize a child before closing it; this must
   not replace its remembered normal geometry.
5. Save a project, quit with both children open, and restart. Confirm no child
   process/window survives quitting, and that restored project state and
   utility-window geometry remain usable. Repeat quitting with unsaved work:
   cancel the quit prompt and confirm both windows and their shared state still
   work, then save/discard through the normal prompt.
6. On Wayland, check size restoration but expect the compositor to choose the
   position/monitor. On other platforms, check placement on both monitors,
   including negative desktop coordinates. After a monitor/DPI configuration
   change, check that the window manager keeps restored windows reachable.
