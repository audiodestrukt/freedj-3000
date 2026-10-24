# ADK-1000 control hub — tscircuit generates, TraceMaker routes, KiCad judges.
#
#   make check     connectivity only (schematic build, no PCB) — fast
#   make build     full tscircuit build (placement, pours; routing is disabled)
#   make export    build/control-board.kicad_pcb from the tscircuit board
#   make route     TraceMaker on the exported board -> build/control-board.routed.kicad_pcb
#   make drc       kicad-cli DRC on the routed board, summary to the terminal
#   make all       export + route + drc
#
# TraceMaker: https://github.com/DingoOz/TraceMaker — put its bin/ on PATH or set TRACEMAKER.
# KiCad 9/10: kicad-cli on PATH.

export PATH := $(HOME)/.bun/bin:$(PATH)
TSCI       ?= ./node_modules/.bin/tsci
TRACEMAKER ?= tracemaker
KICAD_CLI  ?= kicad-cli
SRC         = index.circuit.tsx
OUT         = build/control-board
ROUTE_TIME ?= 120

.PHONY: all check build export route drc clean

all: export route drc

check:
	$(TSCI) build $(SRC) --disable-pcb --schematic-svgs 2>&1 | tee build/check.log | grep -iE "error|warn|missing|no ports|✔|✓" || true

build:
	mkdir -p build
	$(TSCI) build $(SRC) 2>&1 | tee build/build.log | tail -5

export: build
	$(TSCI) export $(SRC) -f kicad_pcb -o $(OUT).kicad_pcb 2>&1 | tail -3
	@python3 -c "import re,sys; s=open('$(OUT).kicad_pcb').read(); print('footprints', s.count('(footprint'), 'nets', len(re.findall(r'\(net \d+ ', s)), 'segments', s.count('(segment'))"

# prep: strip any tscircuit routing, add the bottom GND pour (tscircuit's <copperpour> does not
# export), create the .kicad_pro with JLCPCB's real rules, fill zones.  Needs KiCad's pcbnew module
# (system python3).
PREP ?= /usr/bin/python3 $(HOME)/.claude/skills/pcb-layout/scripts/prep_board.py
prep: export
	$(PREP) $(OUT).kicad_pcb -o $(OUT).prep.kicad_pcb --pour GND:B.Cu --fab jlcpcb 2>&1 | tail -3

route: prep
	cp $(OUT).prep.kicad_pro $(OUT).routed.kicad_pro 2>/dev/null || true
	$(TRACEMAKER) route $(OUT).prep.kicad_pcb -o $(OUT).routed.kicad_pcb --time $(ROUTE_TIME) 2>&1 | tail -15

drc:
	$(KICAD_CLI) pcb drc --format json -o build/drc.json $(OUT).routed.kicad_pcb >/dev/null 2>&1 || true
	@python3 -c "import json,collections; d=json.load(open('build/drc.json')); print('violations', len(d['violations']), 'unconnected', len(d['unconnected_items'])); print(collections.Counter(v['type'] for v in d['violations']).most_common(8))"

clean:
	rm -rf build dist
