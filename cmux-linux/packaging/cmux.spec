# RPM spec for cmux-linux (Fedora).
#
# Build locally with:
#   cd cmux-linux
#   cargo build --release
#   rpmbuild -bb packaging/cmux.spec --define "_sourcedir $(pwd)"
#
# or via the helper: packaging/build-rpm.sh

%global debug_package %{nil}

Name:           cmux
Version:        0.1.0
Release:        1%{?dist}
Summary:        Terminal multiplexer with vertical tabs and agent notifications (Linux port of cmux)

License:        MIT
URL:            https://github.com/manaflow-ai/cmux
# Built from the workspace sources; see packaging/build-rpm.sh.

BuildRequires:  rust >= 1.80
BuildRequires:  cargo
BuildRequires:  gcc
BuildRequires:  pkgconfig(gtk+-3.0)
BuildRequires:  pkgconfig(webkit2gtk-4.1)
BuildRequires:  pkgconfig(libxdo)

Requires:       gtk3
Requires:       webkit2gtk4.1

%description
cmux-linux is a Fedora-native reimplementation of cmux: a terminal multiplexer
for AI coding agents, featuring a vertical-tab sidebar, split panes arranged in
a binary split tree, per-pane notification rings, a notification feed, and a
control socket with a `cmux` CLI so agents and scripts can drive the app.

The GUI is built with Dioxus (Rust) over WebKitGTK; terminal panes are backed by
real PTYs and a built-in VT/ANSI emulator.

%prep
# Sources are expected to already be present in %{_sourcedir} (the workspace).

%build
cd %{_sourcedir}
cargo build --release --locked

%install
install -d %{buildroot}%{_bindir}
install -m 0755 %{_sourcedir}/target/release/cmux-gui %{buildroot}%{_bindir}/cmux-gui
install -m 0755 %{_sourcedir}/target/release/cmux     %{buildroot}%{_bindir}/cmux

install -d %{buildroot}%{_datadir}/applications
install -m 0644 %{_sourcedir}/packaging/cmux.desktop %{buildroot}%{_datadir}/applications/cmux.desktop

install -d %{buildroot}%{_userunitdir}
install -m 0644 %{_sourcedir}/packaging/cmux-daemon.service %{buildroot}%{_userunitdir}/cmux-daemon.service

install -d %{buildroot}%{_datadir}/icons/hicolor/scalable/apps
install -m 0644 %{_sourcedir}/packaging/cmux.svg %{buildroot}%{_datadir}/icons/hicolor/scalable/apps/cmux.svg

%files
%license %{_sourcedir}/../LICENSE
%{_bindir}/cmux-gui
%{_bindir}/cmux
%{_datadir}/applications/cmux.desktop
%{_userunitdir}/cmux-daemon.service
%{_datadir}/icons/hicolor/scalable/apps/cmux.svg

%changelog
* Sat Jun 14 2026 cmux-linux contributors - 0.1.0-1
- Initial Fedora package: GUI (cmux-gui), CLI (cmux), desktop + systemd integration.
