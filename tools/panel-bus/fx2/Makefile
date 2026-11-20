# Firmware for the fx2 logic analyser (CY7C68013A) as a panel-link master.
# Needs sdcc (brew install sdcc) and pyusb; see README.md.
SDCC = sdcc -mmcs51 --code-size 0x3e00 --xram-loc 0x3e00 --xram-size 0x100

all: ring.ihx blink.ihx bootlog.ihx
%.ihx: %.c
	$(SDCC) $< -o $@
load-%: %.ihx
	python3 load.py $<
clean:
	rm -f *.ihx *.lk *.map *.mem *.rel *.rst *.sym *.asm *.lst
.PHONY: all clean
