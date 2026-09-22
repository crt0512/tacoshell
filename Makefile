# tacoshell makefile
#
#   make [CONFIG=path/to/app.toml] [build|install|uninstall|deb|pkg|apk|apk-legacy|clean]
#
#   CONFIG   the app to build, default tacoshell.toml
#   TARGET   rust target triple to build for, default this machine
#            (aarch64-unknown-linux-gnu, x86_64-pc-windows-gnu, ...)
#   GLIBC    oldest glibc the linux build should run on, like 2.31
#
# make apk takes its own, TARGET and GLIBC dont go there:
#   ANDROID_TARGETS  rust targets that go in the apk, default aarch64-linux-android (every phone of the last years).
#                    armv7-linux-androideabi for old 32 bit phones, x86_64-linux-android i686-linux-android for emulators
#                    (those two untested). rustup target add them first, several go into the same apk
#   ANDROID_MIN_SDK  oldest android it installs on, default 24 (android 7)
#   ANDROID_KEYSTORE key to sign with, default the debug key android studio uses (made if there is none yet).
#                    ANDROID_KEY_ALIAS and ANDROID_KEYSTORE_PASS go with it, apksigner asks for the password otherwise
#   ANDROID_HOME     the sdk, default where android studio puts it. wants build-tools 35+, a platform and the ndk
# make apk-legacy is an apk for android 4.0.3 and up (32 bit arm, opengl es 2), see below
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

ANDROID_HOME          ?= $(or $(ANDROID_SDK_ROOT),$(HOME)/Android/Sdk)
ANDROID_TARGETS       ?= aarch64-linux-android
ANDROID_MIN_SDK       ?= 24
# 35 and up forces edge to edge, the system bars would then sit on top of the terminal and the kiosk bar
ANDROID_TARGET_SDK    ?= 34
# has to go up with every release or android wont install it over the old one. default: from the version, 1.2.3 -> 1002003
ANDROID_VERSION_CODE  ?=
ANDROID_KEYSTORE      ?=
ANDROID_KEY_ALIAS     ?=
ANDROID_KEYSTORE_PASS ?=
# what apk-legacy changes, set by it
ANDROID_NDK_API       ?= $(ANDROID_MIN_SDK)
ANDROID_FEATURES      ?=
ANDROID_LINK_ARGS     ?=
ANDROID_SYMBOLS       ?=
APK_SUFFIX            ?=

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
MAC_ICON := $(dir $(CONFIG))logo.icns
PKG_ENV  := $(OUT)/tacoshell-package.env
DEB_ROOT := $(OUT)/deb-root
MAC_ROOT := $(OUT)/mac-root
APK_DIR  := $(OUT)/apk
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

# ---- android sdk bits, only looked up when make apk uses them ----

# newest of whatever the glob finds, by version (build-tools/9.0.0 isnt newer than 36.0.0)
newest = $(shell ls -d $(1) 2>/dev/null | sort -V | tail -n 1)
ANDROID_NDK         ?= $(or $(ANDROID_NDK_HOME),$(call newest,$(ANDROID_HOME)/ndk/*))
ANDROID_BUILD_TOOLS ?= $(call newest,$(ANDROID_HOME)/build-tools/*)
ANDROID_JAR         ?= $(call newest,$(ANDROID_HOME)/platforms/android-*/android.jar)
ANDROID_NDK_BIN      = $(firstword $(wildcard $(ANDROID_NDK)/toolchains/llvm/prebuilt/*/bin))
DEBUG_KEYSTORE      := $(HOME)/.android/debug.keystore

# rust triple -> android abi, the folder under lib/ in the apk
android_abi = $(strip \
	$(if $(filter aarch64-%,$(1)),arm64-v8a, \
	$(if $(filter armv7-%,$(1)),armeabi-v7a, \
	$(if $(filter x86_64-%,$(1)),x86_64, \
	$(if $(filter i686-%,$(1)),x86,unknown)))))
# the ndk's clang for a triple and api level, it says armv7a where rust says armv7
android_cc = $(ANDROID_NDK_BIN)/$(subst armv7-,armv7a-,$(1))$(ANDROID_NDK_API)-clang
# cargo only finds the ndk through these: the linker, and cc + ar for the c in aws-lc (russh's crypto)
android_env = CARGO_TARGET_$(shell echo $(1) | tr a-z- A-Z_)_LINKER=$(call android_cc,$(1)) \
	CC_$(subst -,_,$(1))=$(call android_cc,$(1)) AR_$(subst -,_,$(1))=$(ANDROID_NDK_BIN)/llvm-ar

# what the .so wants from the system, minus the weak ones (those may be missing)
android_imports = $(ANDROID_NDK_BIN)/llvm-nm -D --undefined-only target/$(1)/release/libtacoshell.so | awk '$$1 != "w" { sub(/@.*/, "", $$2); print $$2 }'

# the app as a .so for one triple. 16k pages because newer phones have those and wont load a lib built for 4k.
# with ANDROID_SYMBOLS everything it imports has to be in there, or android that old wont load it
define android_lib
	$(call android_env,$(1)) $(CARGO) rustc --lib --crate-type cdylib --release --target $(1) $(ANDROID_FEATURES) $(CARGO_FLAGS) -- -C link-arg=-Wl,-z,max-page-size=16384 $(addprefix -C link-arg=,$(ANDROID_LINK_ARGS))
	@[ -z "$(ANDROID_SYMBOLS)" ] || { missing=$$($(call android_imports,$(1)) | grep -vxF -f $(ANDROID_SYMBOLS)); [ -z "$$missing" ] || { echo "android $(ANDROID_MIN_SDK) doesnt have:" $$missing "(stand ins go in src/legacy/libc_shims.rs)" >&2; exit 1; }; }

endef

# how apksigner gets the key: yours, or the debug one with the password everyone knows
APK_SIGN = $(if $(ANDROID_KEYSTORE),--ks $(ANDROID_KEYSTORE) $(if $(ANDROID_KEY_ALIAS),--ks-key-alias $(ANDROID_KEY_ALIAS)) \
	$(if $(ANDROID_KEYSTORE_PASS),--ks-pass env:ANDROID_KEYSTORE_PASS),--ks $(DEBUG_KEYSTORE) --ks-key-alias androiddebugkey --ks-pass pass:android)

.PHONY: all build install uninstall deb pkg apk apk-legacy clean _package_env _desktop _mac_app _apk

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
	@# use the app's checked-in macos icon when available, otherwise make one from the baked-in png
	@if [ -f "$(MAC_ICON)" ]; then \
		install -m 644 "$(MAC_ICON)" "$(APP_DEST)/$(call meta,NAME).app/Contents/Resources/icon.icns"; \
	elif [ -f "$(ICON)" ]; then \
		rm -rf $(OUT)/icon.iconset && mkdir $(OUT)/icon.iconset && \
		for s in 16 32 128 256; do \
			sips -z $$s $$s $(ICON) --out $(OUT)/icon.iconset/icon_$${s}x$${s}.png >/dev/null; \
			sips -z $$((s*2)) $$((s*2)) $(ICON) --out $(OUT)/icon.iconset/icon_$${s}x$${s}@2x.png >/dev/null; \
		done && \
		iconutil -c icns $(OUT)/icon.iconset -o "$(APP_DEST)/$(call meta,NAME).app/Contents/Resources/icon.icns"; \
	fi
	$(FILL) $(PKG_ENV) packaging/Info.plist.in > "$(APP_DEST)/$(call meta,NAME).app/Contents/Info.plist"

# ---- android ----

# one .so per triple, the apk itself out of the first one's build dir (metadata and icon are the same for all)
apk:
	@test -x "$(ANDROID_NDK_BIN)/clang" || { echo "no ndk in $(ANDROID_HOME)/ndk: android studio, sdk manager, SDK Tools, NDK (or set ANDROID_NDK_HOME)" >&2; exit 1; }
	@test -x "$(ANDROID_BUILD_TOOLS)/aapt2" || { echo "no build-tools in $(ANDROID_HOME)/build-tools, set ANDROID_HOME to the android sdk" >&2; exit 1; }
	@test -f "$(ANDROID_JAR)" || { echo "no platform in $(ANDROID_HOME)/platforms: android studio, sdk manager, SDK Platforms" >&2; exit 1; }
	@$(if $(filter unknown,$(foreach t,$(ANDROID_TARGETS),$(call android_abi,$(t)))),echo "ANDROID_TARGETS: dont know the android abi of $(ANDROID_TARGETS)" >&2; exit 1,true)
	$(foreach t,$(ANDROID_TARGETS),$(call android_lib,$(t)))
	$(MAKE) --no-print-directory _apk OUT=target/$(firstword $(ANDROID_TARGETS))/release

# android 4.0.3 and up (api 15): 32 bit arm only, opengl es 2 through src/legacy instead of wgpu, and stand ins for the
# few libc functions api 15 lacks. the ndk only goes down to 21 so it links against that, android-15-symbols.txt makes
# sure nothing newer than 15 got in. the old linker only reads the sysv hash table, not the gnu one
apk-legacy:
	$(MAKE) --no-print-directory apk ANDROID_TARGETS=armv7-linux-androideabi ANDROID_MIN_SDK=15 ANDROID_NDK_API=21 \
		ANDROID_FEATURES="--no-default-features --features legacy" ANDROID_LINK_ARGS=-Wl,--hash-style=both \
		ANDROID_SYMBOLS=packaging/android-15-symbols.txt APK_SUFFIX=-legacy

_apk: export ANDROID_KEYSTORE_PASS := $(ANDROID_KEYSTORE_PASS)
_apk:
	rm -rf $(APK_DIR)
	$(foreach t,$(ANDROID_TARGETS),install -D -m 644 target/$(t)/release/libtacoshell.so $(APK_DIR)/lib/$(call android_abi,$(t))/libtacoshell.so;)
	@# values out of the files again. the label gets xml escaped, and a leading @ or ? would be a reference to aapt2
	@get() { sed -n "s/^$$1=//p" $(META); }; \
	code="$(ANDROID_VERSION_CODE)"; \
	[ -n "$$code" ] || code=$$(get VERSION | awk -F'[^0-9]+' '{ n = $$1 * 1000000 + $$2 * 1000 + $$3; print (n > 0 ? n : 1) }'); \
	{ cat $(META); \
	  echo "LABEL=$$(get NAME | sed 's/&/\&amp;/g; s/</\&lt;/g; s/>/\&gt;/g; s/"/\&quot;/g; s/^[@?]/\\&/')"; \
	  echo "VERSION_CODE=$$code"; \
	  echo "MIN_SDK=$(ANDROID_MIN_SDK)"; echo "TARGET_SDK=$(ANDROID_TARGET_SDK)"; \
	  if [ -f $(ICON) ]; then echo "ICON_RES=@mipmap/icon"; else echo "ICON_RES=@android:drawable/sym_def_app_icon"; fi; \
	} > $(APK_DIR)/values.env
	$(FILL) $(APK_DIR)/values.env packaging/AndroidManifest.xml.in > $(APK_DIR)/AndroidManifest.xml
	@# the icon as the biggest density there is, the launcher scales it down
	if [ -f $(ICON) ]; then \
		install -D -m 644 $(ICON) $(APK_DIR)/res/mipmap-xxxhdpi/icon.png && \
		$(ANDROID_BUILD_TOOLS)/aapt2 compile --dir $(APK_DIR)/res -o $(APK_DIR)/res.zip; \
	fi
	$(ANDROID_BUILD_TOOLS)/aapt2 link -o $(APK_DIR)/unsigned.apk -I $(ANDROID_JAR) \
		--manifest $(APK_DIR)/AndroidManifest.xml $(if $(wildcard $(ICON)),$(APK_DIR)/res.zip)
	@# libs stored, not compressed, and 16k aligned (-P 16, build-tools 35+) so android can map them straight out of the apk
	cd $(APK_DIR) && zip -q -0 -r -D -X unsigned.apk lib
	$(ANDROID_BUILD_TOOLS)/zipalign -f -P 16 4 $(APK_DIR)/unsigned.apk $(APK_DIR)/aligned.apk
	@# the debug key made the way android studio would, if theres no key of your own
	@if [ -z "$(ANDROID_KEYSTORE)" ] && [ ! -f $(DEBUG_KEYSTORE) ]; then \
		echo "making the debug key $(DEBUG_KEYSTORE)"; \
		install -d $(dir $(DEBUG_KEYSTORE)) && \
		keytool -genkeypair -keystore $(DEBUG_KEYSTORE) -storepass android -keypass android -alias androiddebugkey \
			-dname "CN=Android Debug,O=Android,C=US" -keyalg RSA -keysize 2048 -validity 10000 >/dev/null; \
	fi
	$(ANDROID_BUILD_TOOLS)/apksigner sign $(APK_SIGN) --v4-signing-enabled false \
		--out target/$(call meta,BINARY)-$(call meta,VERSION)$(APK_SUFFIX).apk $(APK_DIR)/aligned.apk
	@echo "built target/$(call meta,BINARY)-$(call meta,VERSION)$(APK_SUFFIX).apk (adb install -r it onto a phone)"

clean:
	$(CARGO) clean
