.PHONY: build frontend-build frontend-install frontend-test go-test test

# Application builds must go through Wails. A plain `go build` cannot supply
# Wails' platform-specific tags, CGO setup, or native linker flags.
build:
	go run github.com/wailsapp/wails/v2/cmd/wails@v2.13.0 build \
		-clean \
		-nopackage \
		-trimpath \
		-windowsconsole

frontend-install:
	cd frontend && npm ci

frontend-test:
	cd frontend && npm test

frontend-build:
	cd frontend && npm run build

go-test:
	go test ./...

test: frontend-install frontend-test frontend-build go-test
