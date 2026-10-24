/**
 * ADK-1000 control hub — RP2350A, USB-C (class-compliant USB MIDI), jog
 * encoder input, pitch-fader ADC, browse encoder, I2C panel bus for PCF8574
 * button/LED modules.  Design: docs/design/control-board.md.
 *
 * One flat board (one net scope, no false shorts on export).  tscircuit does
 * the schematic, part selection and PLACEMENT; routing is disabled here and
 * done on the exported .kicad_pcb by TraceMaker, then checked with kicad-cli
 * (see Makefile).  Placement is the whole routing strategy: every part sits
 * on the QFN edge its signals leave, connectors sit on the board edge nearest
 * the pins they serve.
 *
 *   RP2350A edges (as imported):  TOP-left QSPI, TOP-right VREG, TOP-mid USB,
 *                                 BOTTOM XIN/XOUT + RUN, LEFT GPIO0-15,
 *                                 RIGHT GPIO16-29 + ADC.
 */
import React from "react"
import { RP2350A } from "./imports/RP2350A"
import { W25Q16JVSNIQ } from "./imports/W25Q16JVSNIQ"
import { X322512MSB4SI } from "./imports/X322512MSB4SI"
import { TYPE_C_31_M_12 } from "./imports/TYPE_C_31_M_12"
import { USBLC6_2SC6 } from "./imports/USBLC6_2SC6"
import { AMS1117_3_3 } from "./imports/AMS1117_3_3"
import { Decap } from "./lib/place"

// board: 70 x 56 mm, origin at the centre.  Edges at x = ±35, y = ±28.
const W = 70, H = 56
const EDGE_X = W / 2 - 3, EDGE_Y = H / 2 - 3      // connector pin-row centres
const U1 = { x: 2, y: -2 }                          // RP2350A origin

export default () => (
  <board width={`${W}mm`} height={`${H}mm`} routingDisabled>

    {/* ───────────── MCU core (RP2350A + flash + crystal + buck) ───────────── */}
    <RP2350A name="U1" pcbX={U1.x} pcbY={U1.y} />
    <via name="GV1" connectsTo="net.GND" fromLayer="top" toLayer="bottom" pcbX={U1.x + 0.7} pcbY={U1.y + 0.7} />
    <via name="GV2" connectsTo="net.GND" fromLayer="top" toLayer="bottom" pcbX={U1.x - 0.7} pcbY={U1.y - 0.7} />

    {/* QSPI flash above-left, at the QSPI pins */}
    <W25Q16JVSNIQ name="U2" pcbX={U1.x - 2} pcbY={U1.y + 11} />
    <capacitor name="C11" capacitance="100nF" footprint="0402" pcbX={U1.x - 9} pcbY={U1.y + 14} />
    <resistor name="R2" resistance="10k" footprint="0402" pcbX={U1.x - 8} pcbY={U1.y + 12} />
    <resistor name="R9" resistance="1k" footprint="0402" pcbX={U1.x - 12} pcbY={U1.y + 12} />
    <pushbutton name="SW_BOOT" footprint="pushbutton" pcbX={-16} pcbY={24} />

    {/* core buck above-right, at the VREG pins */}
    <inductor name="L1" inductance="3.3uH" footprint="0805" pcbX={11} pcbY={3} />
    <capacitor name="C7" capacitance="10uF" footprint="0805" pcbX={15} pcbY={3} />
    <capacitor name="C8" capacitance="100nF" footprint="0402" pcbX={11} pcbY={6} />
    <capacitor name="C9" capacitance="1uF" footprint="0402" pcbX={14} pcbY={6} />

    {/* 12 MHz crystal below, at XIN/XOUT; 1k in series with XOUT per the design guide */}
    <X322512MSB4SI name="Y1" pcbX={U1.x - 1} pcbY={U1.y - 12.5} />
    <capacitor name="C17" capacitance="15pF" footprint="0402" pcbX={U1.x - 5.5} pcbY={U1.y - 12.5} />
    <capacitor name="C18" capacitance="15pF" footprint="0402" pcbX={U1.x + 3.5} pcbY={U1.y - 12.5} />
    <resistor name="R10" resistance="1k" footprint="0402" pcbX={U1.x + 1.5} pcbY={U1.y - 9} />

    {/* IOVDD / ADC decoupling under the chip on the bottom side */}
    <Decap name="C1" part="RP2350A" ic={U1} pin="IOVDD1" layer="top" dist={2.6} />
    <Decap name="C2" part="RP2350A" ic={U1} pin="IOVDD2" layer="top" dist={2.6} />
    <Decap name="C3" part="RP2350A" ic={U1} pin="IOVDD3" layer="top" dist={2.6} />
    <Decap name="C4" part="RP2350A" ic={U1} pin="IOVDD4" layer="top" dist={2.6} />
    <Decap name="C5" part="RP2350A" ic={U1} pin="IOVDD5" layer="top" dist={2.6} />
    <Decap name="C6" part="RP2350A" ic={U1} pin="IOVDD6" layer="top" dist={2.6} />
    <Decap name="C10" part="RP2350A" ic={U1} pin="ADC_AVDD" layer="top" dist={5.0} />

    {/* reset below-right, near RUN */}
    <resistor name="R1" resistance="10k" footprint="0402" pcbX={U1.x + 7} pcbY={U1.y - 8} />
    <capacitor name="C19" capacitance="100nF" footprint="0402" pcbX={U1.x + 10} pcbY={U1.y - 8} />
    <pushbutton name="SW_RST" footprint="pushbutton" pcbX={U1.x + 16} pcbY={U1.y - 9} />

    {/* ───────────── power: USB-C → ESD → 5V → LDO → 3V3 (left edge) ───────────── */}
    {/* footprint local +y = pin row (rear), -y = opening; rotation 270 turns the
        opening to -x so it sits flush with the left board edge */}
    <TYPE_C_31_M_12 name="J_USB" pcbX={W / 2 - 6} pcbY={14} pcbRotation={90} />
    <resistor name="R7" resistance="5.1k" footprint="0402" pcbX={24} pcbY={6} />
    <resistor name="R8" resistance="5.1k" footprint="0402" pcbX={24} pcbY={4} />
    <USBLC6_2SC6 name="D1" pcbX={20} pcbY={12} />
    {/* 27 Ω series on D+/D−, between the ESD diode and the chip's USB pins */}
    <resistor name="R11" resistance="27" footprint="0402" pcbX={5} pcbY={8} />
    <resistor name="R12" resistance="27" footprint="0402" pcbX={5} pcbY={10} />
    <AMS1117_3_3 name="U5" pcbX={18} pcbY={22} />
    <capacitor name="C15" capacitance="10uF" footprint="0805" pcbX={25} pcbY={22} />
    <capacitor name="C16" capacitance="22uF" footprint="0805" pcbX={11} pcbY={22} />

    {/* ───────────── jog encoder (right edge, top) ───────────── */}
    {/* 5V, GND, A, B, Z, TOUCH.  NPN open-collector encoder (LPD3806 class):
        4.7k pull-ups to 3V3 give 3.3 V highs with the RP2350's pad Schmitt
        trigger on; a 3.3 V push-pull encoder works too.  Not 5 V push-pull. */}
    <pinheader name="J_JOG" pinCount={6} footprint="pinrow6" pcbRotation={90} pcbX={-EDGE_X} pcbY={21}
      pinLabels={{ pin1: "V5", pin2: "GND", pin3: "A", pin4: "B", pin5: "Z", pin6: "TOUCH" }} />
    <resistor name="R20" resistance="4.7k" footprint="0402" pcbX={-EDGE_X + 6} pcbY={24} />
    <resistor name="R21" resistance="4.7k" footprint="0402" pcbX={-EDGE_X + 6} pcbY={22} />
    <resistor name="R22" resistance="10k" footprint="0402" pcbX={-EDGE_X + 6} pcbY={20} />
    <resistor name="R23" resistance="10k" footprint="0402" pcbX={-EDGE_X + 6} pcbY={18} />

    {/* ───────────── fader + aux pot (right edge, middle) ───────────── */}
    <pinheader name="J_FADER" pinCount={3} footprint="pinrow3" pcbRotation={90} pcbX={EDGE_X} pcbY={-2}
      pinLabels={{ pin1: "V3V3", pin2: "WIPER", pin3: "GND" }} />
    <resistor name="R30" resistance="1k" footprint="0402" pcbX={EDGE_X - 6} pcbY={-1} />
    <capacitor name="C30" capacitance="100nF" footprint="0402" pcbX={EDGE_X - 6} pcbY={-3} />
    <pinheader name="J_AUX" pinCount={3} footprint="pinrow3" pcbRotation={90} pcbX={EDGE_X} pcbY={-12}
      pinLabels={{ pin1: "V3V3", pin2: "WIPER", pin3: "GND" }} />
    <resistor name="R31" resistance="1k" footprint="0402" pcbX={EDGE_X - 6} pcbY={-11} />
    <capacitor name="C31" capacitance="100nF" footprint="0402" pcbX={EDGE_X - 6} pcbY={-13} />

    {/* ───────────── browse encoder EC11 (right edge, bottom) ───────────── */}
    <pinheader name="J_BROWSE" pinCount={5} footprint="pinrow5" pcbRotation={90} pcbX={-EDGE_X} pcbY={7}
      pinLabels={{ pin1: "GND", pin2: "A", pin3: "B", pin4: "SW", pin5: "V3V3" }} />
    <resistor name="R40" resistance="10k" footprint="0402" pcbX={-EDGE_X + 6} pcbY={9} />
    <resistor name="R41" resistance="10k" footprint="0402" pcbX={-EDGE_X + 6} pcbY={7} />
    <resistor name="R42" resistance="10k" footprint="0402" pcbX={-EDGE_X + 6} pcbY={5} />
    <capacitor name="C40" capacitance="100nF" footprint="0402" pcbX={-EDGE_X + 9} pcbY={9} />
    <capacitor name="C41" capacitance="100nF" footprint="0402" pcbX={-EDGE_X + 9} pcbY={7} />
    <capacitor name="C42" capacitance="100nF" footprint="0402" pcbX={-EDGE_X + 9} pcbY={5} />

    {/* ───────────── I2C panel bus, two headers in parallel (bottom edge) ───────────── */}
    <pinheader name="J_PANEL1" pinCount={5} footprint="pinrow5" pcbX={-14} pcbY={-EDGE_Y}
      pinLabels={{ pin1: "V3V3", pin2: "GND", pin3: "SDA", pin4: "SCL", pin5: "INT" }} />
    <pinheader name="J_PANEL2" pinCount={5} footprint="pinrow5" pcbX={0} pcbY={-EDGE_Y}
      pinLabels={{ pin1: "V3V3", pin2: "GND", pin3: "SDA", pin4: "SCL", pin5: "INT" }} />
    <resistor name="R50" resistance="4.7k" footprint="0402" pcbX={-12} pcbY={-18} />
    <resistor name="R51" resistance="4.7k" footprint="0402" pcbX={-10} pcbY={-18} />
    <resistor name="R52" resistance="10k" footprint="0402" pcbX={-8} pcbY={-18} />

    {/* ───────────── debug: SWD + UART (bottom edge, right) ───────────── */}
    <pinheader name="J_DBG" pinCount={6} footprint="pinrow6" pcbX={16} pcbY={-EDGE_Y}
      pinLabels={{ pin1: "V3V3", pin2: "GND", pin3: "SWCLK", pin4: "SWDIO", pin5: "TX", pin6: "RX" }} />

    {/* ───────────── spare GPIO 11–18 (top edge) ───────────── */}
    <pinheader name="J_GPIO" pinCount={10} footprint="pinrow10" pcbX={-EDGE_X} pcbY={-15} pcbRotation={90}
      pinLabels={{ pin1: "V3V3", pin2: "GND", pin3: "G11", pin4: "G12", pin5: "G13", pin6: "G14", pin7: "G15", pin8: "G16", pin9: "G17", pin10: "G18" }} />

    {/* status LED */}
    <resistor name="R60" resistance="1k" footprint="0402" pcbX={-9} pcbY={25} />
    <led name="LED1" footprint="0603" color="green" pcbX={-6} pcbY={25} />

    {/* ════════════════════════ nets ════════════════════════ */}

    {/* USB-C: VBUS, GND, shield, CC pull-downs, D+/D− (both orientations) */}
    <trace from="J_USB.A4B9" to="net.VBUS" /><trace from="J_USB.B4A9" to="net.VBUS" />
    <trace from="J_USB.A1B12" to="net.GND" /><trace from="J_USB.B1A12" to="net.GND" />
    <trace from="J_USB.EH1" to="net.GND" /><trace from="J_USB.EH2" to="net.GND" />
    <trace from="J_USB.EH3" to="net.GND" /><trace from="J_USB.EH4" to="net.GND" />
    <trace from="J_USB.A5" to="R7.pin1" /><trace from="R7.pin2" to="net.GND" />
    <trace from="J_USB.B5" to="R8.pin1" /><trace from="R8.pin2" to="net.GND" />
    <trace from="J_USB.A6" to="net.USB_DP_C" /><trace from="J_USB.B6" to="net.USB_DP_C" />
    <trace from="J_USB.A7" to="net.USB_DM_C" /><trace from="J_USB.B7" to="net.USB_DM_C" />
    {/* ESD: pins 1/6 = I/O1, 3/4 = I/O2, 2 = GND, 5 = VBUS */}
    <trace from="D1.pin1" to="net.USB_DP_C" /><trace from="D1.pin6" to="net.USB_DP_C" />
    <trace from="D1.pin3" to="net.USB_DM_C" /><trace from="D1.pin4" to="net.USB_DM_C" />
    <trace from="D1.pin2" to="net.GND" /><trace from="D1.pin5" to="net.VBUS" />
    <trace from="net.USB_DP_C" to="R11.pin1" /><trace from="R11.pin2" to="U1.USB_DP" />
    <trace from="net.USB_DM_C" to="R12.pin1" /><trace from="R12.pin2" to="U1.USB_DM" />

    {/* LDO */}
    <trace from="U5.VIN" to="net.VBUS" />
    <trace from="U5.VOUT1" to="net.V3V3" /><trace from="U5.VOUT2" to="net.V3V3" />
    <trace from="U5.GND" to="net.GND" />
    <trace from="C15.pin1" to="net.VBUS" /><trace from="C15.pin2" to="net.GND" />
    <trace from="C16.pin1" to="net.V3V3" /><trace from="C16.pin2" to="net.GND" />

    {/* MCU power */}
    <trace from="U1.IOVDD1" to="net.V3V3" /><trace from="U1.IOVDD2" to="net.V3V3" />
    <trace from="U1.IOVDD3" to="net.V3V3" /><trace from="U1.IOVDD4" to="net.V3V3" />
    <trace from="U1.IOVDD5" to="net.V3V3" /><trace from="U1.IOVDD6" to="net.V3V3" />
    <trace from="U1.USB_OTP_VDD" to="net.V3V3" /><trace from="U1.QSPI_IOVDD" to="net.V3V3" />
    <trace from="U1.ADC_AVDD" to="net.V3V3" /><trace from="U1.VREG_VIN" to="net.V3V3" />
    <trace from="U1.VREG_AVDD" to="net.V3V3" />
    <trace from="U1.GND" to="net.GND" /><trace from="U1.VREG_PGND" to="net.GND" />
    <trace from="U1.VREG_LX" to="L1.pin1" /><trace from="L1.pin2" to="net.V1V1" />
    <trace from="U1.VREG_FB" to="net.V1V1" />
    <trace from="U1.DVDD1" to="net.V1V1" /><trace from="U1.DVDD2" to="net.V1V1" /><trace from="U1.DVDD3" to="net.V1V1" />
    <trace from="C7.pin1" to="net.V1V1" /><trace from="C7.pin2" to="net.GND" />
    <trace from="C8.pin1" to="net.V1V1" /><trace from="C8.pin2" to="net.GND" />
    <trace from="C9.pin1" to="net.V3V3" /><trace from="C9.pin2" to="net.GND" />
    <trace from="C1.pin1" to="net.V3V3" /><trace from="C1.pin2" to="net.GND" />
    <trace from="C2.pin1" to="net.V3V3" /><trace from="C2.pin2" to="net.GND" />
    <trace from="C3.pin1" to="net.V3V3" /><trace from="C3.pin2" to="net.GND" />
    <trace from="C4.pin1" to="net.V3V3" /><trace from="C4.pin2" to="net.GND" />
    <trace from="C5.pin1" to="net.V3V3" /><trace from="C5.pin2" to="net.GND" />
    <trace from="C6.pin1" to="net.V3V3" /><trace from="C6.pin2" to="net.GND" />
    <trace from="C10.pin1" to="net.V3V3" /><trace from="C10.pin2" to="net.GND" />

    {/* crystal */}
    <trace from="U1.XIN" to="Y1.OSC1" />
    <trace from="U1.XOUT" to="R10.pin1" /><trace from="R10.pin2" to="Y1.OSC2" />
    <trace from="Y1.GND1" to="net.GND" /><trace from="Y1.GND2" to="net.GND" />
    <trace from="C17.pin1" to="Y1.OSC1" /><trace from="C17.pin2" to="net.GND" />
    <trace from="C18.pin1" to="Y1.OSC2" /><trace from="C18.pin2" to="net.GND" />

    {/* reset / boot */}
    <trace from="U1.RUN" to="R1.pin1" /><trace from="R1.pin2" to="net.V3V3" />
    <trace from="U1.RUN" to="C19.pin1" /><trace from="C19.pin2" to="net.GND" />
    <trace from="U1.RUN" to="SW_RST.pin1" /><trace from="SW_RST.pin2" to="net.GND" />

    {/* QSPI flash; BOOTSEL = hold CS low through 1k at power-up */}
    <trace from="U1.QSPI_SD1" to="U2.pin2" /><trace from="U1.QSPI_SD2" to="U2.pin3" />
    <trace from="U1.QSPI_SS" to="U2.CS" /><trace from="U1.QSPI_SCLK" to="U2.CLK" />
    <trace from="U1.QSPI_SD0" to="U2.pin5" /><trace from="U1.QSPI_SD3" to="U2.pin7" />
    <trace from="U2.VCC" to="net.V3V3" /><trace from="U2.GND" to="net.GND" />
    <trace from="C11.pin1" to="net.V3V3" /><trace from="C11.pin2" to="net.GND" />
    <trace from="U2.CS" to="R2.pin1" /><trace from="R2.pin2" to="net.V3V3" />
    <trace from="U2.CS" to="R9.pin1" /><trace from="R9.pin2" to="SW_BOOT.pin1" />
    <trace from="SW_BOOT.pin2" to="net.GND" />

    {/* I2C0 panel bus: GPIO0 SDA, GPIO1 SCL, GPIO2 INT */}
    <trace from="U1.GPIO0" to="net.SDA" /><trace from="U1.GPIO1" to="net.SCL" /><trace from="U1.GPIO2" to="net.PANEL_INT" />
    <trace from="R50.pin1" to="net.SDA" /><trace from="R50.pin2" to="net.V3V3" />
    <trace from="R51.pin1" to="net.SCL" /><trace from="R51.pin2" to="net.V3V3" />
    <trace from="R52.pin1" to="net.PANEL_INT" /><trace from="R52.pin2" to="net.V3V3" />
    <trace from="J_PANEL1.V3V3" to="net.V3V3" /><trace from="J_PANEL1.GND" to="net.GND" />
    <trace from="J_PANEL1.SDA" to="net.SDA" /><trace from="J_PANEL1.SCL" to="net.SCL" /><trace from="J_PANEL1.INT" to="net.PANEL_INT" />
    <trace from="J_PANEL2.V3V3" to="net.V3V3" /><trace from="J_PANEL2.GND" to="net.GND" />
    <trace from="J_PANEL2.SDA" to="net.SDA" /><trace from="J_PANEL2.SCL" to="net.SCL" /><trace from="J_PANEL2.INT" to="net.PANEL_INT" />

    {/* jog: GPIO3 A, GPIO4 B, GPIO5 Z, GPIO6 TOUCH */}
    <trace from="J_JOG.V5" to="net.VBUS" /><trace from="J_JOG.GND" to="net.GND" />
    <trace from="J_JOG.A" to="net.JOG_A" /><trace from="J_JOG.B" to="net.JOG_B" />
    <trace from="J_JOG.Z" to="net.JOG_Z" /><trace from="J_JOG.TOUCH" to="net.JOG_TOUCH" />
    <trace from="U1.GPIO3" to="net.JOG_A" /><trace from="U1.GPIO4" to="net.JOG_B" />
    <trace from="U1.GPIO5" to="net.JOG_Z" /><trace from="U1.GPIO6" to="net.JOG_TOUCH" />
    <trace from="R20.pin1" to="net.JOG_A" /><trace from="R20.pin2" to="net.V3V3" />
    <trace from="R21.pin1" to="net.JOG_B" /><trace from="R21.pin2" to="net.V3V3" />
    <trace from="R22.pin1" to="net.JOG_Z" /><trace from="R22.pin2" to="net.V3V3" />
    <trace from="R23.pin1" to="net.JOG_TOUCH" /><trace from="R23.pin2" to="net.V3V3" />

    {/* browse encoder: GPIO7 A, GPIO8 B, GPIO9 SW */}
    <trace from="J_BROWSE.GND" to="net.GND" /><trace from="J_BROWSE.V3V3" to="net.V3V3" />
    <trace from="J_BROWSE.A" to="net.BR_A" /><trace from="J_BROWSE.B" to="net.BR_B" /><trace from="J_BROWSE.SW" to="net.BR_SW" />
    <trace from="U1.GPIO7" to="net.BR_A" /><trace from="U1.GPIO8" to="net.BR_B" /><trace from="U1.GPIO9" to="net.BR_SW" />
    <trace from="R40.pin1" to="net.BR_A" /><trace from="R40.pin2" to="net.V3V3" />
    <trace from="R41.pin1" to="net.BR_B" /><trace from="R41.pin2" to="net.V3V3" />
    <trace from="R42.pin1" to="net.BR_SW" /><trace from="R42.pin2" to="net.V3V3" />
    <trace from="C40.pin1" to="net.BR_A" /><trace from="C40.pin2" to="net.GND" />
    <trace from="C41.pin1" to="net.BR_B" /><trace from="C41.pin2" to="net.GND" />
    <trace from="C42.pin1" to="net.BR_SW" /><trace from="C42.pin2" to="net.GND" />

    {/* fader ADC0 (GPIO26), aux ADC1 (GPIO27): 1k + 100nF at the pin */}
    <trace from="J_FADER.V3V3" to="net.V3V3" /><trace from="J_FADER.GND" to="net.GND" />
    <trace from="J_FADER.WIPER" to="R30.pin1" /><trace from="R30.pin2" to="net.FADER" />
    <trace from="C30.pin1" to="net.FADER" /><trace from="C30.pin2" to="net.GND" />
    <trace from="U1.GPIO26_ADC0" to="net.FADER" />
    <trace from="J_AUX.V3V3" to="net.V3V3" /><trace from="J_AUX.GND" to="net.GND" />
    <trace from="J_AUX.WIPER" to="R31.pin1" /><trace from="R31.pin2" to="net.AUX" />
    <trace from="C31.pin1" to="net.AUX" /><trace from="C31.pin2" to="net.GND" />
    <trace from="U1.GPIO27_ADC1" to="net.AUX" />

    {/* status LED on GPIO10 */}
    <trace from="U1.GPIO10" to="R60.pin1" /><trace from="R60.pin2" to="LED1.anode" /><trace from="LED1.cathode" to="net.GND" />

    {/* spare GPIO 11–18 */}
    <trace from="J_GPIO.V3V3" to="net.V3V3" /><trace from="J_GPIO.GND" to="net.GND" />
    <trace from="U1.GPIO11" to="J_GPIO.G11" /><trace from="U1.GPIO12" to="J_GPIO.G12" />
    <trace from="U1.GPIO13" to="J_GPIO.G13" /><trace from="U1.GPIO14" to="J_GPIO.G14" />
    <trace from="U1.GPIO15" to="J_GPIO.G15" /><trace from="U1.GPIO16" to="J_GPIO.G16" />
    <trace from="U1.GPIO17" to="J_GPIO.G17" /><trace from="U1.GPIO18" to="J_GPIO.G18" />

    {/* debug: SWD + UART0 on GPIO20/21 */}
    <trace from="J_DBG.V3V3" to="net.V3V3" /><trace from="J_DBG.GND" to="net.GND" />
    <trace from="J_DBG.SWCLK" to="U1.SWCLK" /><trace from="J_DBG.SWDIO" to="U1.SWDIO" />
    <trace from="J_DBG.TX" to="U1.GPIO20" /><trace from="J_DBG.RX" to="U1.GPIO21" />

    <copperpour connectsTo="net.GND" layer="bottom" />
  </board>
)
