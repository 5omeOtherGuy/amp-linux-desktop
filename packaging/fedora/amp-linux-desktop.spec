# LiveKit's prebuilt libwebrtc that Huddles link: WEBRTC_TAG in the webrtc-sys-build version that
# Cargo.lock pins. Keep it equal to _webrtc in packaging/arch/PKGBUILD.
%global webrtc webrtc-a545de4
# The release profile carries no debug info to split out.
%global debug_package %{nil}
# webrtc-sys compiles its C++ bridge with clang only (libwebrtc uses Chromium's libc++); the
# default toolchain exports CXX=g++. LTO objects would not link through rustc, as on Arch (!lto).
%global toolchain clang
%global _lto_cflags %{nil}

Name:           amp-linux-desktop
Version:        0.1.1
Release:        1%{?dist}
Summary:        Native GTK desktop app for Amp
License:        MIT
URL:            https://github.com/5omeOtherGuy/amp-linux-desktop
# Built from the pushed commit by .copr/Makefile (git archive, cargo vendor, and the libwebrtc
# download), so that the build itself needs no network.
Source0:        %{name}-%{version}.tar.gz
Source1:        %{name}-%{version}-vendor.tar.gz
Source2:        %{webrtc}-linux-x64-release.zip
# The prebuilt libwebrtc exists for x86_64 here.
ExclusiveArch:  x86_64
BuildRequires:  cargo
BuildRequires:  rust
BuildRequires:  clang
BuildRequires:  unzip
BuildRequires:  desktop-file-utils
BuildRequires:  pkgconfig(gtk4)
BuildRequires:  pkgconfig(libadwaita-1)
BuildRequires:  pkgconfig(webkitgtk-6.0)
BuildRequires:  pkgconfig(libsoup-3.0)
BuildRequires:  pkgconfig(gstreamer-1.0)
BuildRequires:  pkgconfig(gstreamer-app-1.0)
BuildRequires:  pkgconfig(sqlite3)
Requires:       7zip
Requires:       hicolor-icon-theme
Recommends:     gstreamer1-plugins-good
Recommends:     gstreamer1-plugins-base
Recommends:     pipewire-gstreamer
Recommends:     gstreamer1-plugin-openh264

%description
Amp in a native GTK window, with menus, settings, notifications, a tray menu,
desktop-wide actions, dictation, Huddles and a local runner. Amp's icons are
downloaded from its macOS disk image on first start; no Amp artwork is
included in this package.

%prep
%autosetup
tar -xzf %{SOURCE1}
unzip -q %{SOURCE2} -d webrtc
mkdir -p .cargo
cat >.cargo/config.toml <<EOF
[source.crates-io]
replace-with = "vendored-sources"

[source.vendored-sources]
directory = "vendor"
EOF

%build
export LK_CUSTOM_WEBRTC=$PWD/webrtc/linux-x64-release
cargo build --frozen --release

%install
install -Dm755 target/release/amp-linux-desktop %{buildroot}%{_bindir}/amp-linux-desktop
sed 's|@BIN@|%{_bindir}/amp-linux-desktop|g' data/dev.ampnative.Amp.desktop > dev.ampnative.Amp.desktop
desktop-file-install --dir=%{buildroot}%{_datadir}/applications dev.ampnative.Amp.desktop

%check
desktop-file-validate dev.ampnative.Amp.desktop

%files
%license LICENSE
%doc README.md CHANGELOG.md
%{_bindir}/amp-linux-desktop
%{_datadir}/applications/dev.ampnative.Amp.desktop

%changelog
* Fri Nov 06 2026 Simon Gies <simon.gies@pm.me> - 0.1.1-1
- Initial Fedora package; fetch Amp's icons on first start.
