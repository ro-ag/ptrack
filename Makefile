.PHONY: build test

# Application builds must go through Wails. A plain `go build` cannot supply
# Wails' platform-specific tags, CGO setup, or native linker flags.
build:
	go run github.com/wailsapp/wails/v2/cmd/wails@v2.13.0 build \
		-clean \
		-nopackage \
		-trimpath \
		-windowsconsole

test:
	go test ./...
