/**
 * Power module — USB-C 5V in -> AMS1117-3.3 -> 3V3 rail. Passes USB D+/D-
 * through to the MCU. Outputs break out to J_PWR.
 * Uses a real JLCPCB USB-C receptacle (TYPE-C-31-M-12) so it has a footprint.
 */
import React from "react"
import { AMS1117_3_3 } from "../imports/AMS1117_3_3"
import { TYPE_C_31_M_12 } from "../imports/TYPE_C_31_M_12"

export const PowerBlock = ({ name = "power", pcbX = 0, pcbY = 0 }: any) => (
  <subcircuit name={name} pcbX={pcbX} pcbY={pcbY} autorouter="sequential-trace">
    <TYPE_C_31_M_12 name="USBC" pcbX={-14} pcbY={0} />
    <resistor name="R7" resistance="5.1k" footprint="0402" pcbX={-4} pcbY={7} />
    <resistor name="R8" resistance="5.1k" footprint="0402" pcbX={-4} pcbY={4} />
    <AMS1117_3_3 name="U5" pcbX={3} pcbY={0} />
    <capacitor name="C15" capacitance="10uF" footprint="0805" pcbX={-1} pcbY={-7} />
    <capacitor name="C16" capacitance="22uF" footprint="0805" pcbX={9} pcbY={-7} />

    <pinheader name="J_PWR" pinCount={4} footprint="pinrow4" pcbX={15} pcbY={0}
      pinLabels={{ pin1: "V3V3", pin2: "GND", pin3: "USB_DP", pin4: "USB_DM" }} />

    {/* USB-C power + CC + shield */}
    <trace from="USBC.A4B9" to="net.VBUS" /><trace from="USBC.B4A9" to="net.VBUS" />
    <trace from="USBC.A1B12" to="net.GND" /><trace from="USBC.B1A12" to="net.GND" />
    <trace from="USBC.EH1" to="net.GND" /><trace from="USBC.EH2" to="net.GND" />
    <trace from="USBC.EH3" to="net.GND" /><trace from="USBC.EH4" to="net.GND" />
    <trace from="USBC.A5" to="R7.pin1" /><trace from="R7.pin2" to="net.GND" />
    <trace from="USBC.B5" to="R8.pin1" /><trace from="R8.pin2" to="net.GND" />
    {/* D+/D- (both orientations tied) */}
    <trace from="USBC.A6" to="net.USB_DP" /><trace from="USBC.B6" to="net.USB_DP" />
    <trace from="USBC.A7" to="net.USB_DM" /><trace from="USBC.B7" to="net.USB_DM" />

    {/* LDO */}
    <trace from="U5.VIN" to="net.VBUS" />
    <trace from="U5.VOUT1" to="net.V3V3" /><trace from="U5.VOUT2" to="net.V3V3" />
    <trace from="U5.GND" to="net.GND" />
    <trace from="C15.pin1" to="net.VBUS" /><trace from="C15.pin2" to="net.GND" />
    <trace from="C16.pin1" to="net.V3V3" /><trace from="C16.pin2" to="net.GND" />

    {/* breakout */}
    <trace from="J_PWR.V3V3" to="net.V3V3" /><trace from="J_PWR.GND" to="net.GND" />
    <trace from="J_PWR.USB_DP" to="net.USB_DP" /><trace from="J_PWR.USB_DM" to="net.USB_DM" />

    <copperpour connectsTo="net.GND" layer="bottom" />
  </subcircuit>
)

export default () => (
  <board width="40mm" height="28mm"><PowerBlock /></board>
)
