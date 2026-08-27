# FreeDJ-3000 — build and run
#
#   make            list targets
#   make run        build release and play the default track
#   make two-deck   run a deck plus a simulated second deck over ProDJ Link
#
# Override the track on any run target:
#   make run TRACK=~/music/something.flac

BIN        := target/release/opendeck
CARGO      := cargo
PKG        := opendeck-app
TRACK      ?= techno.mp3

# Second-deck simulation (ProDJ Link beat sender)
BPM        ?= 130.0
HOST       ?= 127.0.0.1
PORT       ?= 50001

# Virtual CDJ harness (prolink-cpp); see docs/reference/link-test-harness.md
PROLINK    ?= $(HOME)/sandbox/thirdparty/prolink-cpp/build
IFACE      ?= eno1
VCDJ_DEV   ?= 5
VCDJ_NAME  ?= VirtualCDJ

# Manual page range to extract for local visual reference
REF_PDF    := reference/pioneer/CDJ-3000X_manual.pdf
REF_URL    := https://downloads.support.alphatheta.com/manuals/dj-players/CDJ-3000X/CDJ-3000X_DRI1956B_manual.pdf

RUST_LOG   ?= info,wgpu=warn,naga=warn

# iOS / iPad app (macOS host only; see ios/README.md).
#   IOS_SIM     simulator name or UDID   (xcrun simctl list devices)
#   IOS_DEVICE  a real iPad's identifier (xcrun devicectl list devices);
#               defaults to the first connected iPad
#   MULTICAST=1 include the Link multicast entitlement — only once Apple has
#               granted it for the App ID, otherwise signing fails
IOS_CONFIG    ?= Debug
IOS_SIM       ?= iPad Pro 11-inch (M4)
IOS_BUNDLE_ID ?= com.audiodestruct.opendeck
IOS_DEVICE    ?= $(shell xcrun devicectl list devices 2>/dev/null | grep -i ipad | \
                   grep -oE '[0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{12}' | head -1)
IOS_ENT        = $(if $(MULTICAST),FREEDJ_ENTITLEMENTS=freedj/freedj.entitlements,)
# Only override the track if TRACK actually names a file — otherwise let
# bundle-track.sh do its own look in the repo root (TRACK's default, techno.mp3,
# is gitignored and often absent).
IOS_TRACK      = $(if $(wildcard $(TRACK)),FREEDJ_TRACK="$(abspath $(TRACK))",)
IOS_SIM_APP    = $(call ios_app,iphonesimulator)
IOS_APP        = $(call ios_app,iphoneos)
# Ask xcodebuild where it put the .app rather than hardcoding a DerivedData path.
ios_app        = $(shell cd ios && xcodebuild -project freedj.xcodeproj -scheme freedj \
                   -configuration $(IOS_CONFIG) -sdk $(1) -showBuildSettings 2>/dev/null \
                   | awk '/ BUILT_PRODUCTS_DIR =/{print $$3}')/freedj.app

.DEFAULT_GOAL := help
.PHONY: help build debug relink ios ios-device ios-sim run chrome dev two-deck link-pair beat virtual-cdj shot check fmt clippy test perf clean reference distclean

## ── Build ──────────────────────────────────────────────────────────────────

build: ## Release build (this is what you want for audio)
	$(CARGO) build --release -p $(PKG)

debug: ## Debug build — faster to compile, audio may glitch
	$(CARGO) build -p $(PKG)

relink: ## Force a relink — fixes "librubberband.so.N: cannot open shared object file"
	@touch crates/timestretch/build.rs crates/app/src/main.rs
	@rm -f $(BIN)
	$(CARGO) build --release -p $(PKG)
	@ldd $(BIN) | grep -E 'rubberband|not found' || true

## ── iOS (macOS host only; see ios/README.md) ───────────────────────────────

ios: ## Build the signed iPad app, bundling TRACK into it
	cd ios && xcodebuild -project freedj.xcodeproj -scheme freedj \
	    -configuration $(IOS_CONFIG) -sdk iphoneos -arch arm64 \
	    -allowProvisioningUpdates $(IOS_TRACK) $(IOS_ENT) build

ios-device: ios ## Install + launch on a connected iPad, streaming its log
	@test -n "$(IOS_DEVICE)" || { echo "no iPad found — connect one over USB-C, or set IOS_DEVICE="; exit 1; }
	xcrun devicectl device install app --device $(IOS_DEVICE) "$(IOS_APP)"
	xcrun devicectl device process launch --console --terminate-existing \
	    --device $(IOS_DEVICE) $(IOS_BUNDLE_ID)

ios-sim: ## Build + run the iPad app in IOS_SIM (see: xcrun simctl list devices)
	cd ios && xcodebuild -project freedj.xcodeproj -scheme freedj \
	    -configuration $(IOS_CONFIG) -sdk iphonesimulator -arch arm64 \
	    CODE_SIGNING_ALLOWED=NO build
	@test -f "$(TRACK)" || { echo "no such track: $(TRACK) — set TRACK=path/to/file.mp3"; exit 1; }
	@cp "$(TRACK)" "$(IOS_SIM_APP)/$(notdir $(TRACK))"
	xcrun simctl bootstatus "$(IOS_SIM)" -b
	xcrun simctl install "$(IOS_SIM)" "$(IOS_SIM_APP)"
	xcrun simctl launch --console-pty "$(IOS_SIM)" $(IOS_BUNDLE_ID)

## ── Run ────────────────────────────────────────────────────────────────────

run: build ## Play TRACK (default: techno.mp3)
	@test -f "$(TRACK)" || { echo "no such track: $(TRACK)"; echo "usage: make run TRACK=path/to/file.mp3"; exit 1; }
	RUST_LOG=$(RUST_LOG) ./$(BIN) "$(TRACK)"

chrome: build ## Play TRACK with the full deck faceplate (jog, fader, buttons)
	@test -f "$(TRACK)" || { echo "no such track: $(TRACK)"; echo "usage: make chrome TRACK=path/to/file.mp3"; exit 1; }
	RUST_LOG=$(RUST_LOG) ./$(BIN) "$(TRACK)" --faceplate

portrait: build ## Local iPad 13" portrait chrome dev loop (TRACK optional, empty deck if none)
	RUST_LOG=$(RUST_LOG) ./$(BIN) $(if $(wildcard $(TRACK)),"$(TRACK)",) --portrait

dev: debug ## Play TRACK with debug logging (verbose: MIDI + ProDJ packets)
	@test -f "$(TRACK)" || { echo "no such track: $(TRACK)"; exit 1; }
	RUST_LOG=debug,wgpu=warn,naga=warn ./target/debug/opendeck "$(TRACK)"

## ── Two-deck testing ───────────────────────────────────────────────────────

two-deck: build ## Run a deck + a simulated CDJ sending beats at BPM
	@test -f "$(TRACK)" || { echo "no such track: $(TRACK)"; exit 1; }
	@echo "Deck A: $(TRACK)"
	@echo "Deck B: simulated CDJ at $(BPM) BPM -> $(HOST):$(PORT)"
	@echo "Match deck A's pitch to $(BPM) and watch the cyan strip lock."
	@python3 tools/send_beat.py $(BPM) $(HOST) $(PORT) & \
	  SENDER=$$!; \
	  trap "kill $$SENDER 2>/dev/null" EXIT INT TERM; \
	  RUST_LOG=$(RUST_LOG) ./$(BIN) "$(TRACK)"

link-pair: build ## Two freedj instances (players 1 and 2) linked to each other — TRACK, TRACK2
	@test -f "$(TRACK)" || { echo "no such track: $(TRACK)"; exit 1; }
	@T2="$(TRACK2)"; test -n "$$T2" || T2="$(TRACK)"; \
	 echo "player 1: $(TRACK)"; echo "player 2: $$T2"; \
	 RUST_LOG=$(RUST_LOG) ./$(BIN) "$(TRACK)" --player 1 --deck A & P1=$$!; \
	 trap "kill $$P1 2>/dev/null" EXIT INT TERM; \
	 sleep 1; RUST_LOG=$(RUST_LOG) ./$(BIN) "$$T2" --player 2 --deck B

beat: ## Send ProDJ Link beat packets only (no deck) — BPM=130.0
	python3 tools/send_beat.py $(BPM) $(HOST) $(PORT)

virtual-cdj: ## Run prolink-cpp's virtual CDJ on IFACE (announce+beat+status) — BPM, VCDJ_DEV
	@test -x $(PROLINK)/prolink_virtual_cdj || { echo "prolink_virtual_cdj not built in $(PROLINK) — see docs/reference/link-test-harness.md"; exit 1; }
	@IP=$$(ip -4 -o addr show $(IFACE) | awk '{print $$4}' | cut -d/ -f1); \
	 BC=$$(ip -4 -o addr show $(IFACE) | awk '{print $$6}'); \
	 test -n "$$IP" || { echo "no IPv4 on $(IFACE) — set IFACE=..."; exit 1; }; \
	 echo "virtual CDJ: device $(VCDJ_DEV) '$(VCDJ_NAME)' @ $(BPM) BPM on $$IP (bcast $$BC)"; \
	 sleep 100000 | $(PROLINK)/prolink_virtual_cdj $$IP $$BC 02:fd:00:00:00:0$(VCDJ_DEV) $(VCDJ_DEV) $(VCDJ_NAME) $(BPM)

## ── Screenshots ────────────────────────────────────────────────────────────

SHOT ?= docs/screenshots/playback.png

shot: build ## Capture the playback screen to SHOT (default docs/screenshots/playback.png)
	@mkdir -p $(dir $(SHOT))
	OPENDECK_SCREENSHOT=$(SHOT) RUST_LOG=opendeck=info,wgpu=off,naga=off,egui=off ./$(BIN) "$(TRACK)" 2>&1 | grep -E "captured|error" || true

shot-portrait: build ## Capture the iPad portrait chrome to SHOT (TRACK optional)
	@mkdir -p $(dir $(SHOT))
	OPENDECK_PORTRAIT=1 OPENDECK_SCREENSHOT=$(SHOT) RUST_LOG=opendeck=info,wgpu=off,naga=off,egui=off ./$(BIN) $(if $(wildcard $(TRACK)),"$(TRACK)",) 2>&1 | grep -E "captured|error" || true

## ── Quality ────────────────────────────────────────────────────────────────

check: ## Type-check the whole workspace
	$(CARGO) check --workspace

fmt: ## Format
	$(CARGO) fmt --all

clippy: ## Lint
	$(CARGO) clippy --workspace -- -D warnings

test: ## Run tests
	$(CARGO) test --workspace

perf: ## Run the DSP real-time-factor guard, printing the measured RTF
	$(CARGO) test --release -p opendeck-timestretch perf -- --nocapture

## ── Reference material ─────────────────────────────────────────────────────

reference: ## Download the CDJ-3000X manual and extract screen pages (local only)
	./tools/fetch-reference.sh

## ── Housekeeping ───────────────────────────────────────────────────────────

clean: ## Remove build artifacts
	$(CARGO) clean

distclean: clean ## Also remove downloaded reference material
	rm -rf reference/pioneer

help: ## Show this help
	@echo "FreeDJ-3000"
	@echo ""
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) \
	  | awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-12s\033[0m %s\n", $$1, $$2}'
	@echo ""
	@echo "Variables:  TRACK=$(TRACK)  BPM=$(BPM)  PORT=$(PORT)"
