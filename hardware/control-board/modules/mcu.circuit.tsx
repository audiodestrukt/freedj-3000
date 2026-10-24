/**
 * MCU module — RP2350A + W25Q16 QSPI flash + 12MHz crystal + core buck +
 * reset/boot. Edge-aware placement: each neighbor sits on the QFN edge its
 * signals exit, so the greedy router gets short, uncrossed paths.
 *
 *   QFN edges:  TOP-left = QSPI, TOP-right = VREG, TOP-mid = USB,
 *               BOTTOM = XIN/XOUT + RUN, LEFT = GPIO/SWD.
 */
import React from "react"
import { RP2350A } from "../imports/RP2350A"
import { W25Q16JVSNIQ } from "../imports/W25Q16JVSNIQ"
import { X322512MSB4SI } from "../imports/X322512MSB4SI"
import { Decap } from "../lib/place"

const U1 = { x: 0, y: 0 } // RP2350A origin in this subcircuit

export const McuBlock = ({ name = "mcu", pcbX = 0, pcbY = 0 }: any) => (
  <subcircuit name={name} pcbX={pcbX} pcbY={pcbY} autorouter="sequential-trace">
    <RP2350A name="U1" pcbX={0} pcbY={0} />

    {/* QSPI flash directly ABOVE-LEFT, facing the top-left QSPI pins */}
    <W25Q16JVSNIQ name="U2" pcbX={-2} pcbY={11} />
    {/* stitch the QFN center thermal pad (GND) down to the bottom pour */}
    <via name="GV1" connectsTo="net.GND" fromLayer="top" toLayer="bottom" pcbX={0.7} pcbY={0.7} />
    <via name="GV2" connectsTo="net.GND" fromLayer="top" toLayer="bottom" pcbX={-0.7} pcbY={-0.7} />
    <capacitor name="C11" capacitance="100nF" footprint="0402" pcbX={-8} pcbY={8} />
    <resistor name="R2" resistance="10k" footprint="0402" pcbX={-8} pcbY={12} />
    <resistor name="R9" resistance="1k" footprint="0402" pcbX={-12} pcbY={12} />
    <pushbutton name="SW_BOOT" footprint="pushbutton" pcbX={-16} pcbY={13} />

    {/* core buck on TOP-RIGHT, at the VREG pins */}
    <inductor name="L1" inductance="3.3uH" footprint="0805" pcbX={4} pcbY={10} />
    <capacitor name="C7" capacitance="10uF" footprint="0805" pcbX={8} pcbY={11} />
    <capacitor name="C8" capacitance="100nF" footprint="0402" pcbX={8} pcbY={8} />
    <capacitor name="C9" capacitance="1uF" footprint="0402" pcbX={4} pcbY={6} />

    {/* 12MHz crystal BELOW, at XIN/XOUT */}
    <X322512MSB4SI name="Y1" pcbX={-1} pcbY={-10} />
    <capacitor name="C17" capacitance="15pF" footprint="0402" pcbX={-5} pcbY={-10} />
    <capacitor name="C18" capacitance="15pF" footprint="0402" pcbX={3} pcbY={-10} />

    {/* IOVDD decoupling — auto-co-located at each power pin via the Decap helper.
        No more hand-guessed coordinates; reproducible across variants. */}
    <Decap name="C1" part="RP2350A" ic={U1} pin="IOVDD1" layer="bottom" />
    <Decap name="C2" part="RP2350A" ic={U1} pin="IOVDD2" layer="bottom" />
    <Decap name="C3" part="RP2350A" ic={U1} pin="IOVDD3" layer="bottom" />
    <Decap name="C4" part="RP2350A" ic={U1} pin="IOVDD4" layer="bottom" />
    <Decap name="C5" part="RP2350A" ic={U1} pin="IOVDD5" layer="bottom" />
    <Decap name="C6" part="RP2350A" ic={U1} pin="IOVDD6" layer="bottom" />

    {/* reset at BOTTOM-RIGHT, near RUN */}
    <resistor name="R1" resistance="10k" footprint="0402" pcbX={5} pcbY={-8} />
    <capacitor name="C19" capacitance="100nF" footprint="0402" pcbX={8} pcbY={-8} />
    <pushbutton name="SW_RST" footprint="pushbutton" pcbX={11} pcbY={-10} />

    {/* I2C pullups on the LEFT, near GPIO0/1 */}
    <resistor name="R3" resistance="4.7k" footprint="0402" pcbX={-9} pcbY={-2} />
    <resistor name="R4" resistance="4.7k" footprint="0402" pcbX={-9} pcbY={-4} />

    {/* breakout header on the LEFT edge (vertical), where GPIO/SWD exit */}
    <pinheader name="J_IO" pinCount={12} footprint="pinrow12" pcbRotation={90} pcbX={-17} pcbY={0}
      pinLabels={{
        pin1: "V3V3", pin2: "GND", pin3: "SDA", pin4: "SCL",
        pin5: "BCK", pin6: "WS", pin7: "DIN", pin8: "IRQ",
        pin9: "LED", pin10: "USB_DP", pin11: "USB_DM", pin12: "SWD",
      }} />

    {/* ---- power ---- */}
    <trace from="J_IO.V3V3" to="net.V3V3" />
    <trace from="U1.IOVDD1" to="net.V3V3" /><trace from="U1.IOVDD2" to="net.V3V3" />
    <trace from="U1.IOVDD3" to="net.V3V3" /><trace from="U1.IOVDD4" to="net.V3V3" />
    <trace from="U1.IOVDD5" to="net.V3V3" /><trace from="U1.IOVDD6" to="net.V3V3" />
    <trace from="U1.USB_OTP_VDD" to="net.V3V3" /><trace from="U1.QSPI_IOVDD" to="net.V3V3" />
    <trace from="U1.ADC_AVDD" to="net.V3V3" /><trace from="U1.VREG_VIN" to="net.V3V3" />
    <trace from="U1.VREG_AVDD" to="net.V3V3" />
    <trace from="U1.GND" to="net.GND" /><trace from="U1.VREG_PGND" to="net.GND" />
    <trace from="U1.VREG_LX" to="L1.pin1" /><trace from="L1.pin2" to="net.V1V1" />
    <trace from="U1.VREG_FB" to="net.V1V1" />
    <trace from="U1.DVDD1" to="net.V1V1" /><trace from="U1.DVDD2" to="net.V1V1" />
    <trace from="U1.DVDD3" to="net.V1V1" />
    <trace from="C7.pin1" to="net.V1V1" /><trace from="C7.pin2" to="net.GND" />
    <trace from="C8.pin1" to="net.V1V1" /><trace from="C8.pin2" to="net.GND" />
    <trace from="C9.pin1" to="net.V3V3" /><trace from="C9.pin2" to="net.GND" />
    <trace from="C1.pin1" to="net.V3V3" /><trace from="C1.pin2" to="net.GND" />
    <trace from="C2.pin1" to="net.V3V3" /><trace from="C2.pin2" to="net.GND" />
    <trace from="C3.pin1" to="net.V3V3" /><trace from="C3.pin2" to="net.GND" />
    <trace from="C4.pin1" to="net.V3V3" /><trace from="C4.pin2" to="net.GND" />
    <trace from="C5.pin1" to="net.V3V3" /><trace from="C5.pin2" to="net.GND" />
    <trace from="C6.pin1" to="net.V3V3" /><trace from="C6.pin2" to="net.GND" />

    {/* ---- crystal ---- */}
    <trace from="U1.XIN" to="Y1.OSC1" /><trace from="U1.XOUT" to="Y1.OSC2" />
    <trace from="Y1.GND1" to="net.GND" /><trace from="Y1.GND2" to="net.GND" />
    <trace from="C17.pin1" to="Y1.OSC1" /><trace from="C17.pin2" to="net.GND" />
    <trace from="C18.pin1" to="Y1.OSC2" /><trace from="C18.pin2" to="net.GND" />

    {/* ---- reset / boot ---- */}
    <trace from="U1.RUN" to="R1.pin1" /><trace from="R1.pin2" to="net.V3V3" />
    <trace from="U1.RUN" to="C19.pin1" /><trace from="C19.pin2" to="net.GND" />
    <trace from="U1.RUN" to="SW_RST.pin1" /><trace from="SW_RST.pin2" to="net.GND" />

    {/* ---- QSPI flash (inner pins SD1/SD2 declared first so the greedy
         router claims their direct paths before the outer pins block them) ---- */}
    <trace from="U1.QSPI_SD1" to="U2.pin2" /><trace from="U1.QSPI_SD2" to="U2.pin3" />
    <trace from="U1.QSPI_SS" to="U2.CS" /><trace from="U1.QSPI_SCLK" to="U2.CLK" />
    <trace from="U1.QSPI_SD0" to="U2.pin5" /><trace from="U1.QSPI_SD3" to="U2.pin7" />
    <trace from="U2.VCC" to="net.V3V3" /><trace from="U2.GND" to="net.GND" />
    <trace from="C11.pin1" to="net.V3V3" /><trace from="C11.pin2" to="net.GND" />
    <trace from="U2.CS" to="R2.pin1" /><trace from="R2.pin2" to="net.V3V3" />
    <trace from="U2.CS" to="R9.pin1" /><trace from="R9.pin2" to="SW_BOOT.pin1" />
    <trace from="SW_BOOT.pin2" to="net.GND" />

    {/* ---- buses to header ---- */}
    <trace from="U1.GPIO0" to="net.SDA" /><trace from="U1.GPIO1" to="net.SCL" />
    <trace from="net.SDA" to="J_IO.SDA" /><trace from="net.SCL" to="J_IO.SCL" />
    <trace from="R3.pin1" to="net.SDA" /><trace from="R3.pin2" to="net.V3V3" />
    <trace from="R4.pin1" to="net.SCL" /><trace from="R4.pin2" to="net.V3V3" />
    <trace from="U1.GPIO3" to="J_IO.BCK" /><trace from="U1.GPIO4" to="J_IO.WS" />
    <trace from="U1.GPIO5" to="J_IO.DIN" /><trace from="U1.GPIO2" to="J_IO.IRQ" />
    <trace from="U1.GPIO6" to="J_IO.LED" />
    <trace from="U1.USB_DP" to="J_IO.USB_DP" /><trace from="U1.USB_DM" to="J_IO.USB_DM" />
    <trace from="U1.SWCLK" to="J_IO.SWD" />
    <trace from="J_IO.GND" to="net.GND" />

    <copperpour connectsTo="net.GND" layer="bottom" />
  </subcircuit>
)

export default () => (
  <board width="50mm" height="46mm"><McuBlock /></board>
)
