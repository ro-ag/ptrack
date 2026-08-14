.PHONY: build frontend-build frontend-install frontend-test exporter-test help-check test \
	package archive dmg icons sign verify-sign signed-dmg notarize release-dmg

# Version and target are explicit so local packages exercise the same identity
# as the tag-only release workflow. Override VERSION for an unsigned candidate.
VERSION ?= $(shell python3 -c 'import tomllib; print(tomllib.load(open("src-tauri/Cargo.toml", "rb"))["package"]["version"])')
RUST_TARGET ?= $(shell rustc -vV | sed -n 's/^host: //p')
CARGO_TARGET_DIR ?= target
UNAME_OS := $(shell uname -s)
UNAME_ARCH := $(shell uname -m)
ARCHIVE_OS := $(if $(filter Darwin,$(UNAME_OS)),darwin,$(if $(filter Linux,$(UNAME_OS)),linux,windows))
ARCHIVE_ARCH := $(if $(filter arm64 aarch64,$(UNAME_ARCH)),arm64,amd64)
APP := $(CARGO_TARGET_DIR)/$(RUST_TARGET)/release/bundle/macos/p-track.app
DMG := dist/p-track_$(VERSION)_darwin_$(ARCHIVE_ARCH).dmg
CLI_ARCHIVE := dist/ptrack_$(VERSION)_darwin_$(ARCHIVE_ARCH).tar.gz
ENTITLEMENTS := build/darwin/entitlements.plist
TAURI := npm --prefix frontend run tauri --

# Use the SHA-1 fingerprint to avoid ambiguous same-name identities.
SIGN_IDENTITY ?= D0F5928D9173891DA0EC4C7A52DCB190E483034C
NOTARY_PROFILE ?= ptrack-notarize

build: frontend-install frontend-build
	PTRACK_BUILD_VERSION="$(VERSION)" CARGO_TARGET_DIR="$(CARGO_TARGET_DIR)" \
		cargo build --locked --release --package ptrack-desktop --bin ptrack \
		--target "$(RUST_TARGET)"

# Build the unsigned native macOS app, retain the internal CLI, and install the
# frozen launcher that always selects `ptrack gui` when Finder opens the app.
package: frontend-install
	PTRACK_BUILD_VERSION="$(VERSION)" CARGO_TARGET_DIR="$(CARGO_TARGET_DIR)" \
		$(TAURI) build --target "$(RUST_TARGET)" --bundles app --no-sign --ci \
		--config '{"version":"$(VERSION)"}'
	cp build/darwin/launcher "$(APP)/Contents/MacOS/p-track"
	chmod +x "$(APP)/Contents/MacOS/p-track"
	/usr/libexec/PlistBuddy -c "Set :CFBundleExecutable p-track" "$(APP)/Contents/Info.plist"
	/usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString $(VERSION)" "$(APP)/Contents/Info.plist"
	/usr/libexec/PlistBuddy -c "Set :CFBundleVersion $(VERSION)" "$(APP)/Contents/Info.plist"
	python3 tools/release_contract.py validate-binary \
		"$(APP)/Contents/MacOS/ptrack" "$(VERSION)" darwin "$(ARCHIVE_ARCH)"

archive: package
	rm -rf build/archive
	mkdir -p build/archive
	cp "$(APP)/Contents/MacOS/ptrack" README.md LICENSE build/archive/
	mkdir -p dist
	tar -C build/archive -czf "$(CLI_ARCHIVE)" .
	rm -rf build/archive

dmg: package
	rm -rf build/dmg
	mkdir -p build/dmg dist
	cp -R "$(APP)" build/dmg/
	ln -s /Applications build/dmg/Applications
	hdiutil create -volname "p-track" -srcfolder build/dmg \
		-ov -format UDZO "$(DMG)"
	rm -rf build/dmg

sign: package
	codesign --force --options runtime --entitlements "$(ENTITLEMENTS)" \
		--sign "$(SIGN_IDENTITY)" --timestamp "$(APP)/Contents/MacOS/ptrack"
	codesign --force --options runtime --entitlements "$(ENTITLEMENTS)" \
		--sign "$(SIGN_IDENTITY)" --timestamp "$(APP)"
	@$(MAKE) --no-print-directory verify-sign

verify-sign:
	codesign --verify --strict --verbose=2 "$(APP)"
	@codesign -dv --verbose=2 "$(APP)" 2>&1 | grep -E "^(Identifier|Authority|TeamIdentifier|Timestamp)" || true

signed-dmg: sign
	rm -rf build/dmg
	mkdir -p build/dmg dist
	cp -R "$(APP)" build/dmg/
	ln -s /Applications build/dmg/Applications
	hdiutil create -volname "p-track" -srcfolder build/dmg -ov -format UDZO "$(DMG)"
	rm -rf build/dmg
	codesign --force --sign "$(SIGN_IDENTITY)" --timestamp "$(DMG)"

notarize: signed-dmg
	@if ! xcrun notarytool history --keychain-profile "$(NOTARY_PROFILE)" >/dev/null 2>&1; then \
		echo "notarytool profile '$(NOTARY_PROFILE)' not found."; \
		echo "Create it with xcrun notarytool store-credentials $(NOTARY_PROFILE) ..."; \
		exit 1; \
	fi
	xcrun notarytool submit "$(DMG)" --keychain-profile "$(NOTARY_PROFILE)" --wait
	xcrun stapler staple "$(DMG)"
	xcrun stapler validate "$(DMG)"

release-dmg: notarize
	hdiutil verify "$(DMG)"
	codesign --verify --strict --verbose=2 \
		-R='anchor apple generic and certificate leaf[field.1.2.840.113635.100.6.1.13] exists and certificate leaf[subject.OU] = "3CAJR4ZDMQ"' "$(DMG)"
	spctl --assess --type open --context context:primary-signature -vv "$(DMG)"

icons:
	python3 assets/brand/generate_icons.py

frontend-install:
	cd frontend && npm ci

frontend-test:
	cd frontend && npm test

frontend-build:
	cd frontend && npm run build

exporter-test:
	cd tools/ptrack-db-export && go test ./...
	cd tools/ptrack-db-export && go vet ./...

help-check:
	python3 -B -m unittest tools.help_check_test tools.release_contract_test
	python3 -B tools/help_check.py all

test: frontend-install frontend-test frontend-build
	cargo fmt --all -- --check
	cargo test --workspace --all-targets --no-fail-fast
	cargo clippy --workspace --all-targets -- -D warnings
	RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps
	cd tools/ptrack-db-export && go test ./...
	cd tools/ptrack-db-export && go vet ./...
	python3 -B -m unittest tools.help_check_test tools.release_contract_test
	python3 -B tools/help_check.py all
