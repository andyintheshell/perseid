Name: perseid
Version: %{app_version}
Release: 1
Summary: A native HTTP(S) security testing workbench
License: MIT OR Apache-2.0
URL: https://github.com/andyintheshell/perseid
# Include libraries loaded dynamically by the desktop backend as well as the
# ELF dependencies detected automatically by rpmbuild.
Requires: libglvnd-egl, libglvnd-glx, mesa-dri-drivers
Requires: libxkbcommon, libxkbcommon-x11, libwayland-client, libwayland-cursor, libwayland-egl
Requires: libX11, libxcb, libXcursor, libXi, libXrandr

%description
A native HTTP(S) security testing workbench.

%install
install -Dm755 %{source_root}/target/release/perseid %{buildroot}/usr/bin/perseid
install -Dm644 %{source_root}/branding/perseid-icon-512.png %{buildroot}/usr/share/icons/hicolor/512x512/apps/perseid.png
install -Dm644 %{source_root}/packaging/perseid.desktop %{buildroot}/usr/share/applications/perseid.desktop
install -Dm644 %{source_root}/LICENSE-MIT %{buildroot}/usr/share/licenses/perseid/LICENSE-MIT
install -Dm644 %{source_root}/LICENSE-APACHE %{buildroot}/usr/share/licenses/perseid/LICENSE-APACHE

%files
/usr/bin/perseid
/usr/share/applications/perseid.desktop
/usr/share/icons/hicolor/512x512/apps/perseid.png
%license /usr/share/licenses/perseid/LICENSE-MIT
%license /usr/share/licenses/perseid/LICENSE-APACHE
