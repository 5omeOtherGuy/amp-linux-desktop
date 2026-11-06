# COPR's make_srpm method runs `make srpm outdir=DIR spec=SPEC` in a clone of this repository at the
# pushed commit. This builds the source tarball from that commit and the source RPM from the spec.
version := $(shell sed -n 's/^VERSION = "\(.*\)"$$/\1/p' amp_native/config.py)
top := $(shell mktemp -d)

srpm:
	dnf -y install git rpm-build
	git config --global --add safe.directory '*'
	mkdir -p $(top)/SOURCES
	git archive --prefix=amp-linux-desktop-$(version)/ -o $(top)/SOURCES/amp-linux-desktop-$(version).tar.gz HEAD
	rpmbuild -bs --define '_topdir $(top)' --define '_srcrpmdir $(outdir)' packaging/fedora/amp-linux-desktop.spec
