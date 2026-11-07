# COPR's make_srpm method runs `make srpm outdir=DIR spec=SPEC` in a clone of this repository at the
# pushed commit. This builds the source tarball from that commit, the vendored crates and LiveKit's
# prebuilt libwebrtc, so that the RPM builds offline, and the source RPM from the spec.
version := $(shell sed -n '/^\[workspace.package\]/,/^\[/ s/^version = "\(.*\)"$$/\1/p' Cargo.toml)
webrtc := $(shell sed -n 's/^%global webrtc //p' packaging/fedora/amp-linux-desktop.spec)
top := $(shell mktemp -d)

srpm:
	dnf -y install git rpm-build cargo curl
	git config --global --add safe.directory '*'
	mkdir -p $(top)/SOURCES
	git archive --prefix=amp-linux-desktop-$(version)/ -o $(top)/SOURCES/amp-linux-desktop-$(version).tar.gz HEAD
	cargo vendor --locked $(top)/vendor >/dev/null
	tar -C $(top) -czf $(top)/SOURCES/amp-linux-desktop-$(version)-vendor.tar.gz vendor
	curl -fsSL -o $(top)/SOURCES/$(webrtc)-linux-x64-release.zip \
		https://github.com/livekit/rust-sdks/releases/download/$(webrtc)/webrtc-linux-x64-release.zip
	rpmbuild -bs --define '_topdir $(top)' --define '_srcrpmdir $(outdir)' packaging/fedora/amp-linux-desktop.spec
