.PHONY: build frontend-build frontend-install frontend-test go-test test \
	package dmg icons sign verify-sign signed-dmg notarize release-dmg

WAILS := go run github.com/wailsapp/wails/v2/cmd/wails@v2.13.0

# Version stamped into the app bundle: latest tag, or "dev" outside a checkout.
VERSION ?= $(shell git describe --tags --always --dirty 2>/dev/null | sed 's/^v//' || echo dev)
ARCH := $(shell uname -m)
APP := build/bin/p-track.app
DMG := build/bin/p-track-$(VERSION)-macOS-$(ARCH).dmg
ENTITLEMENTS := build/darwin/entitlements.plist

# Signing identity: the SHA-1 fingerprint of the Developer ID Application
# certificate. Using the fingerprint instead of the certificate name avoids
# "ambiguous identity" errors when more than one Developer ID certificate
# with the same name exists on the machine. Find it with:
#   security find-identity -v -p codesigning
SIGN_IDENTITY ?= D0F5928D9173891DA0EC4C7A52DCB190E483034C

# notarytool keychain profile holding the App Store Connect API credentials.
# Create it once with:
#   xcrun notarytool store-credentials "$(NOTARY_PROFILE)" \
#     --key AuthKey_XXXX.p8 --key-id <KEY-ID> --issuer <ISSUER-ID>
NOTARY_PROFILE ?= ptrack-notarize

# Application builds must go through Wails. A plain `go build` cannot supply
# Wails' platform-specific tags, CGO setup, or native linker flags.
build: frontend-install frontend-build
	$(WAILS) build \
		-clean \
		-nopackage \
		-trimpath \
		-windowsconsole

# macOS app bundle: build/bin/p-track.app with the branded icon and the
# Info.plist from build/darwin/. The bundle version is stamped from git so
# local builds never silently inherit the version pinned in wails.json. The
# bundle's entry point is the launcher script (build/darwin/launcher), which
# always runs `ptrack gui`; the Wails binary stays available as the plain CLI.
package: frontend-install frontend-build
	$(WAILS) build -clean -trimpath
	cp build/darwin/launcher "$(APP)/Contents/MacOS/p-track"
	chmod +x "$(APP)/Contents/MacOS/p-track"
	/usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString $(VERSION)" "$(APP)/Contents/Info.plist"
	/usr/libexec/PlistBuddy -c "Set :CFBundleVersion $(VERSION)" "$(APP)/Contents/Info.plist"
	@echo "Built $(APP) (version $(VERSION))"

# Distributable disk image with an /Applications drop link.
dmg: package
	rm -rf build/dmg
	mkdir -p build/dmg
	cp -R "$(APP)" build/dmg/
	ln -s /Applications build/dmg/Applications
	hdiutil create -volname "p-track" -srcfolder build/dmg \
		-ov -format UDZO "$(DMG)"
	rm -rf build/dmg
	@echo "Built $(DMG)"

# Sign the app bundle with the Developer ID identity (hardened runtime,
# entitlements, secure timestamp). The ptrack binary gets its own signature
# first: the bundle's main executable is the launcher script, and the notary
# validates each binary individually. Works locally with the identity in your
# login keychain; the first run may ask for key access — choose Always Allow.
sign: package
	codesign --force --options runtime \
		--entitlements "$(ENTITLEMENTS)" \
		--sign "$(SIGN_IDENTITY)" \
		--timestamp \
		"$(APP)/Contents/MacOS/ptrack"
	codesign --force --options runtime \
		--entitlements "$(ENTITLEMENTS)" \
		--sign "$(SIGN_IDENTITY)" \
		--timestamp \
		"$(APP)"
	@$(MAKE) --no-print-directory verify-sign

verify-sign:
	codesign --verify --strict --verbose=2 "$(APP)"
	@codesign -dv --verbose=2 "$(APP)" 2>&1 | grep -E "^(Identifier|Authority|TeamIdentifier|Timestamp)" || true
	@echo "Signature OK: $(APP)"

# Signed (but not notarized) disk image: Gatekeeper still warns on first
# launch, but the Developer ID signature is fully valid.
signed-dmg: sign
	rm -rf build/dmg
	mkdir -p build/dmg
	cp -R "$(APP)" build/dmg/
	ln -s /Applications build/dmg/Applications
	hdiutil create -volname "p-track" -srcfolder build/dmg \
		-ov -format UDZO "$(DMG)"
	rm -rf build/dmg
	codesign --force --sign "$(SIGN_IDENTITY)" --timestamp "$(DMG)"
	@echo "Built signed $(DMG)"

# Notarize the signed DMG and staple the ticket. Requires the notarytool
# keychain profile (see NOTARY_PROFILE above); without it this prints the
# one-time setup command and stops.
notarize: signed-dmg
	@if ! xcrun notarytool history --keychain-profile "$(NOTARY_PROFILE)" >/dev/null 2>&1; then \
		echo "notarytool profile '$(NOTARY_PROFILE)' not found."; \
		echo "Create it once with:"; \
		echo "  xcrun notarytool store-credentials $(NOTARY_PROFILE) \\"; \
		echo "    --key AuthKey_XXXX.p8 --key-id <KEY-ID> --issuer <ISSUER-ID>"; \
		exit 1; \
	fi
	xcrun notarytool submit "$(DMG)" \
		--keychain-profile "$(NOTARY_PROFILE)" --wait
	xcrun stapler staple "$(DMG)"
	xcrun stapler validate "$(DMG)"
	@echo "Notarized and stapled: $(DMG)"

# Full release pipeline: build, sign, DMG, sign, notarize, staple.
release-dmg: notarize
	spctl --assess --type open --context context:primary-signature -vv "$(DMG)"
	@echo "Release-ready: $(DMG)"

# Regenerate build/appicon.png and assets/brand/* from source.
icons:
	python3 assets/brand/generate_icons.py

frontend-install:
	cd frontend && npm ci

frontend-test:
	cd frontend && npm test

frontend-build:
	cd frontend && npm run build

go-test:
	go test ./...

test: frontend-install frontend-test frontend-build go-test
