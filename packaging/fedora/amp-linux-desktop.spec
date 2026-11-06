Name:           amp-linux-desktop
Version:        0.1.1
Release:        1%{?dist}
Summary:        Native GTK desktop app for Amp
License:        MIT
URL:            https://github.com/5omeOtherGuy/amp-linux-desktop
# Built from the pushed commit by .copr/Makefile (git archive), not from a download.
Source0:        %{name}-%{version}.tar.gz
BuildArch:      noarch
BuildRequires:  python3-devel
BuildRequires:  desktop-file-utils
Requires:       python3
Requires:       python3-gobject
Requires:       gtk4
Requires:       libadwaita
Requires:       webkitgtk6.0
Requires:       7zip
Requires:       hicolor-icon-theme
Recommends:     gstreamer1-plugins-good
Recommends:     gstreamer1-plugins-base
Recommends:     pipewire-gstreamer
Recommends:     gstreamer1-plugin-openh264
Recommends:     curl

%description
Amp in a native GTK window, with menus, settings, notifications, a tray menu,
desktop-wide actions and a local runner. Amp's icons are downloaded from its
macOS disk image on first start; no Amp artwork is included in this package.
Huddles require the optional LiveKit Python SDK.

%prep
%autosetup

%build

%install
install -d %{buildroot}%{python3_sitelib}/amp_native
install -m644 amp_native/*.py amp_native/style.css %{buildroot}%{python3_sitelib}/amp_native/
install -Dm755 packaging/arch/amp-linux-desktop.sh %{buildroot}%{_bindir}/amp-linux-desktop
sed 's|@BIN@|%{_bindir}/amp-linux-desktop|g' data/dev.ampnative.Amp.desktop > dev.ampnative.Amp.desktop
desktop-file-install --dir=%{buildroot}%{_datadir}/applications dev.ampnative.Amp.desktop

%check
desktop-file-validate dev.ampnative.Amp.desktop

%files
%license LICENSE
%doc README.md CHANGELOG.md
%{_bindir}/amp-linux-desktop
%{python3_sitelib}/amp_native/
%{_datadir}/applications/dev.ampnative.Amp.desktop

%changelog
* Fri Nov 06 2026 Simon Gies <simon.gies@pm.me> - 0.1.1-1
- Initial Fedora package; fetch Amp's icons on first start.
