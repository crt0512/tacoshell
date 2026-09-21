# tacoshell makefile
#
#   make [CONFIG=path/to/app.toml] [build|install|uninstall|deb|pkg|clean]
#
#   CONFIG   the app to build, default tacoshell.toml
#   TARGET   rust target triple to build for, default this machine
#            (aarch64-unknown-linux-gnu, x86_64-pc-windows-gnu, ...)
#   GLIBC    oldest glibc the linux build should run on, like 2.31
#
# GLIBC and building for another architecture go through cargo-zigbuild, zig does the linking.
# without cargo-zigbuild or zig it falls back to plain cargo build: 
# the binary then wants at least the glibc of this machine, and another architecture needs its own linker
#
# names, ids, the icon: 
# build.rs puts them next to the binary (tacoshell-meta.env, tacoshell-icon.png), 
# so there is no second place to keep in sync and nothing here has to run the binary, which it couldnt for another architecture anyway.

CONFIG      ?= tacoshell.toml
TARGET      ?=
GLIBC       ?=
PREFIX      ?= /usr/local
CARGO       ?= cargo
CARGO_FLAGS ?=
# overrides packaging.maintainer and the git identity fallback
MAINTAINER  ?=

export TACOSHELL_CONFIG := $(abspath $(CONFIG))

HOST := $(shell rustc -vV | sed -n 's/^host: //p')

# zig only when theres something for it to do and it is actually there
HAVE_ZIG := $(shell command -v cargo-zigbuild >/dev/null 2>&1 && command -v zig >/dev/null 2>&1 && echo yes)
WANT_ZIG := $(or $(GLIBC),$(filter-out $(HOST),$(TARGET)))
USE_ZIG  := $(and $(WANT_ZIG),$(HAVE_ZIG))
ifneq ($(and $(WANT_ZIG),$(if $(HAVE_ZIG),,no)),)
$(warning cargo-zigbuild or zig not found, building with plain cargo:$(if $(GLIBC), GLIBC=$(GLIBC) is ignored and the binary needs this machine's glibc;)$(if $(filter-out $(HOST),$(TARGET)), $(TARGET) needs a linker for it set up in cargo))
endif

# a glibc floor needs an explicit target for zig, this machine's if none was given
ifneq ($(GLIBC),)
TARGET := $(or $(TARGET),$(HOST))
endif

OUT      := target/$(if $(TARGET),$(TARGET)/)release
BIN      := $(OUT)/tacoshell$(if $(findstring windows,$(TARGET)),.exe)
META     := $(OUT)/tacoshell-meta.env
ICON     := $(OUT)/tacoshell-icon.png
PKG_ENV  := $(OUT)/tacoshell-package.env
DEB_ROOT := $(OUT)/deb-root
MAC_ROOT := $(OUT)/mac-root
FILL     := sh packaging/fill.sh

# only the steps that write outside the tree get sudo, building stays yours
ifeq ($(strip $(DESTDIR)),)
SUDO := $(shell if [ "$$(id -u)" = 0 ]; then echo; elif command -v sudo >/dev/null 2>&1; then echo sudo; fi)
else
SUDO :=
endif

# $(call meta,KEY) -> value from the metadata build.rs wrote. only valid after `build`
meta = $(shell sed -n 's/^$(1)=//p' $(META) 2>/dev/null)

# rust triple -> debian architecture
deb_arch = $(strip \
	$(if $(filter x86_64-%,$(1)),amd64, \
	$(if $(filter aarch64-%,$(1)),arm64, \
	$(if $(filter armv7-% arm-%,$(1)),armhf, \
	$(if $(filter i686-% i586-%,$(1)),i386, \
	$(if $(filter riscv64%,$(1)),riscv64,unknown))))))
ARCH := $(if $(TARGET),$(call deb_arch,$(TARGET)),$(shell dpkg --print-architecture 2>/dev/null || echo amd64))

# the window stuff isnt linked, it gets opened at runtime (egl for drawing, x11 or wayland
# for the window, xkbcommon for the keyboard), so no tool finds these on its own
GUI_DEPENDS := libegl1, libxkbcommon0, libxkbcommon-x11-0, libx11-6, libx11-xcb1, \
	libxcursor1, libxi6, libwayland-client0, libwayland-egl1

.PHONY: all build install uninstall deb pkg clean _package_env _desktop _mac_app

all: build

build:
	$(CARGO) $(if $(USE_ZIG),zigbuild,build) --release $(if $(TARGET),--target $(TARGET)$(if $(and $(USE_ZIG),$(GLIBC)),.$(GLIBC))) $(CARGO_FLAGS)

# everything the templates in packaging/ get filled with: the metadata plus what only
# exists once theres a binary (glibc it needs, architecture, maintainer, dates).
# values come out of the files, never through make, so quotes in them cant break the shell
_package_env: export TACO_MAINTAINER := $(MAINTAINER)
_package_env:
	@test -f $(META) || { echo "no $(META), build first" >&2; exit 1; }
	@get() { sed -n "s/^$$1=//p" $(META); }; \
	glibc=$$(readelf -V $(BIN) 2>/dev/null | grep -oE 'GLIBC_[0-9]+(\.[0-9]+)+' | sed 's/GLIBC_//' | sort -Vu | tail -1); \
	maintainer=$${TACO_MAINTAINER:-$$(get MAINTAINER)}; \
	if [ -z "$$maintainer" ]; then \
		n=$$(git config user.name); m=$$(git config user.email); \
		if [ -n "$$n" ] && [ -n "$$m" ]; then maintainer="$$n <$$m>"; else maintainer="Unknown <unknown@localhost>"; fi; \
	fi; \
	depends="libc6 (>= $${glibc:-2.17}), libgcc-s1, $$(echo '$(GUI_DEPENDS)' | tr -s ' ')"; \
	extra=$$(get DEPENDS); [ -n "$$extra" ] && depends="$$depends, $$extra"; \
	license=$$(get LICENSE); icon_name=; [ -n "$$(get ICON_SIZE)" ] && icon_name=$$(get ID); \
	{ cat $(META); \
	  echo "ARCH=$(ARCH)"; \
	  echo "MAINTAINER=$$maintainer"; \
	  echo "LICENSE=$${license:-unknown}"; \
	  echo "DEPENDS_ALL=$$depends"; \
	  echo "ICON_NAME=$$icon_name"; \
	  echo "DATE=$$(date -R)"; echo "DATE_SHORT=$$(date +%Y-%m-%d)"; echo "YEAR=$$(date +%Y)"; \
	} > $(PKG_ENV)

# ---- install (linux) ----

install: build _package_env
	$(SUDO) install -d $(DESTDIR)$(PREFIX)/bin
	$(SUDO) install -m 755 $(BIN) $(DESTDIR)$(PREFIX)/bin/$(call meta,BINARY)
ifeq ($(shell uname), Darwin)
	@echo "on macos use 'make pkg' for an app bundle"
else
	$(MAKE) --no-print-directory _desktop ROOT=$(DESTDIR) SUDO_CMD="$(SUDO)"
endif
	@echo "installed $(call meta,NAME) as $(PREFIX)/bin/$(call meta,BINARY)"

uninstall: build
	$(SUDO) rm -f $(DESTDIR)$(PREFIX)/bin/$(call meta,BINARY)
	$(SUDO) rm -f $(DESTDIR)$(PREFIX)/share/applications/$(call meta,ID).desktop
	$(SUDO) rm -f $(DESTDIR)$(PREFIX)/share/icons/hicolor/*/apps/$(call meta,ID).png
	$(SUDO) rm -f $(DESTDIR)$(PREFIX)/share/man/man1/$(call meta,BINARY).1.gz
	@echo "uninstalled $(call meta,NAME)"

# desktop entry (with a kiosk launcher action), icon and man page under $(ROOT)$(PREFIX)
_desktop:
	$(SUDO_CMD) install -d $(ROOT)$(PREFIX)/share/applications $(ROOT)$(PREFIX)/share/man/man1
	$(FILL) $(PKG_ENV) packaging/desktop.in > $(OUT)/app.desktop
	$(SUDO_CMD) install -m 644 $(OUT)/app.desktop $(ROOT)$(PREFIX)/share/applications/$(call meta,ID).desktop
	@if [ -n "$(call meta,ICON_SIZE)" ]; then \
		$(SUDO_CMD) install -d $(ROOT)$(PREFIX)/share/icons/hicolor/$(call meta,ICON_SIZE)/apps; \
		$(SUDO_CMD) install -m 644 $(ICON) $(ROOT)$(PREFIX)/share/icons/hicolor/$(call meta,ICON_SIZE)/apps/$(call meta,ID).png; \
	fi
	$(FILL) $(PKG_ENV) packaging/man.1.in | gzip -9n > $(OUT)/app.1.gz
	$(SUDO_CMD) install -m 644 $(OUT)/app.1.gz $(ROOT)$(PREFIX)/share/man/man1/$(call meta,BINARY).1.gz

# ---- debian ----

deb: build _package_env
	rm -rf $(DEB_ROOT)
	install -d $(DEB_ROOT)/usr/bin $(DEB_ROOT)/DEBIAN $(DEB_ROOT)/usr/share/doc/$(call meta,BINARY)
	install -m 755 $(BIN) $(DEB_ROOT)/usr/bin/$(call meta,BINARY)
	$(MAKE) --no-print-directory _desktop ROOT=$(abspath $(DEB_ROOT)) PREFIX=/usr SUDO_CMD= TARGET=$(TARGET) GLIBC=$(GLIBC)
	$(FILL) $(PKG_ENV) packaging/copyright.in > $(DEB_ROOT)/usr/share/doc/$(call meta,BINARY)/copyright
	$(FILL) $(PKG_ENV) packaging/changelog.in | gzip -9n > $(DEB_ROOT)/usr/share/doc/$(call meta,BINARY)/changelog.gz
	@# redirection takes the umask, debian wants 644 no matter whose machine built it
	chmod 644 $(DEB_ROOT)/usr/share/doc/$(call meta,BINARY)/*
	@# control last, Installed-Size is whatever ended up in the tree
	echo "INSTALLED_SIZE=$$(du -sk --exclude=DEBIAN $(DEB_ROOT) | cut -f1)" >> $(PKG_ENV)
	$(FILL) $(PKG_ENV) packaging/control.in > $(DEB_ROOT)/DEBIAN/control
	@# the long description, folded and indented the way debian wants it
	sed -n 's/^LONG_DESCRIPTION=//p' $(META) | fold -s -w 76 | sed 's/ *$$//; s/^/ /' >> $(DEB_ROOT)/DEBIAN/control
	dpkg-deb --build --root-owner-group $(DEB_ROOT) target/$(call meta,BINARY)_$(call meta,VERSION)_$(ARCH).deb
	@echo "built target/$(call meta,BINARY)_$(call meta,VERSION)_$(ARCH).deb"

# ---- macos ----

pkg: build _package_env
	rm -rf $(MAC_ROOT)
	$(MAKE) --no-print-directory _mac_app APP_DEST=$(MAC_ROOT) TARGET=$(TARGET)
	pkgbuild --component "$(MAC_ROOT)/$(call meta,NAME).app" --identifier $(call meta,ID) \
		--version $(call meta,VERSION) --install-location /Applications \
		"target/$(call meta,BINARY)-$(call meta,VERSION)$(if $(TARGET),-$(firstword $(subst -, ,$(TARGET)))).pkg"
	rm -rf $(MAC_ROOT)
	@echo "built target/$(call meta,BINARY)-$(call meta,VERSION)$(if $(TARGET),-$(firstword $(subst -, ,$(TARGET)))).pkg"

_mac_app:
	@test -n "$(APP_DEST)" || (echo "APP_DEST not set" && exit 1)
	install -d "$(APP_DEST)/$(call meta,NAME).app/Contents/MacOS" "$(APP_DEST)/$(call meta,NAME).app/Contents/Resources"
	install -m 755 $(BIN) "$(APP_DEST)/$(call meta,NAME).app/Contents/MacOS/$(call meta,BINARY)"
	@# icns out of the baked in png, only if there is one
	@if [ -f $(ICON) ]; then \
		rm -rf $(OUT)/icon.iconset && mkdir $(OUT)/icon.iconset && \
		for s in 16 32 128 256; do \
			sips -z $$s $$s $(ICON) --out $(OUT)/icon.iconset/icon_$${s}x$${s}.png >/dev/null; \
			sips -z $$((s*2)) $$((s*2)) $(ICON) --out $(OUT)/icon.iconset/icon_$${s}x$${s}@2x.png >/dev/null; \
		done && \
		iconutil -c icns $(OUT)/icon.iconset -o "$(APP_DEST)/$(call meta,NAME).app/Contents/Resources/icon.icns"; \
	fi
	$(FILL) $(PKG_ENV) packaging/Info.plist.in > "$(APP_DEST)/$(call meta,NAME).app/Contents/Info.plist"

clean:
	$(CARGO) clean
