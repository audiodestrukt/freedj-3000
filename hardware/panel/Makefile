# ADK-1000 panel board — layout.toml → gen.py → tscircuit → KiCad → TraceMaker → DRC.
#
#   make gen       regenerate index.circuit.tsx from layout.toml
#   make check     schematic connectivity only
#   make all       gen + export + prep + route + drc
#
# Same toolchain as ../control-board (see its Makefile and docs/design/control-board.md).

export PATH := $(HOME)/.bun/bin:$(PATH)
TSCI       ?= ./node_modules/.bin/tsci
TRACEMAKER ?= tracemaker
KICAD_CLI  ?= kicad-cli
PREP       ?= /usr/bin/python3 $(HOME)/.claude/skills/pcb-layout/scripts/prep_board.py
SRC         = index.circuit.tsx
OUT         = build/panel
ROUTE_TIME ?= 300
# The full-face board is 298 x 380 mm: TraceMaker's automatic lattice needed 20 GB and was
# OOM-killed.  A 0.1 mm lattice without cost fields routes it in a few GB.
ROUTE_FLAGS ?= --pitch-um 100 --variants 2 --threads 2 --fast-bends --no-field --no-gpu
MEM_KB     ?= 16000000

.PHONY: all gen check build export prep route drc render clean

all: gen export prep route drc

gen:
	python3 gen.py

check: gen
	mkdir -p build
	$(TSCI) build $(SRC) --disable-pcb --schematic-svgs 2>&1 | tee build/check.log | grep -iE "error|warn|missing|no ports|✔|✓|Done" || true

build: gen
	mkdir -p build
	$(TSCI) build $(SRC) 2>&1 | tee build/build.log | grep -iE "overlap|error|Done" | head -20

export: build
	$(TSCI) export $(SRC) -f kicad_pcb -o $(OUT).kicad_pcb 2>&1 | tail -1
	@python3 -c "import re; s=open('$(OUT).kicad_pcb').read(); print('footprints', s.count('(footprint'), 'nets', len(re.findall(r'\(net \d+ ', s)))"

render:
	$(TSCI) export $(SRC) -f pcb-svg -o $(OUT).pcb.svg >/dev/null 2>&1; convert -background black -density 60 $(OUT).pcb.svg $(OUT).pcb.png && echo "$(OUT).pcb.png"

prep: export
	$(PREP) $(OUT).kicad_pcb -o $(OUT).prep.kicad_pcb --pour GND:B.Cu --fab jlcpcb 2>&1 | tail -1

route: prep
	cp $(OUT).prep.kicad_pro $(OUT).routed.kicad_pro 2>/dev/null || true
	( ulimit -v $(MEM_KB); $(TRACEMAKER) route $(OUT).prep.kicad_pcb -o $(OUT).routed.kicad_pcb --time $(ROUTE_TIME) $(ROUTE_FLAGS) 2>&1 | grep -E "^routed|unrouted|<- best|bad_alloc" | head -20 )

drc:
	$(KICAD_CLI) pcb drc --format json -o build/drc.json $(OUT).routed.kicad_pcb >/dev/null 2>&1 || true
	@python3 -c "import json,collections; d=json.load(open('build/drc.json')); print('violations', len(d['violations']), 'unconnected', len(d['unconnected_items'])); print(collections.Counter(v['type'] for v in d['violations']).most_common(8))"

clean:
	rm -rf build dist
