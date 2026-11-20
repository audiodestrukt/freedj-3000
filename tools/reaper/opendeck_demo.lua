-- OpenDeck in REAPER: the deck on a track, its Pitch automated: hold, ramp
-- up to +8 %, ramp down to -8 %, an LFO, back to 0, looped every 50 s.
--
--   OPENDECK_TRACK=/path/to/track.mp3 OPENDECK_AUTOPLAY=1 reaper -new tools/reaper/opendeck_demo.lua
--
-- (make plugin first).  See docs/design/plugin.md.
reaper.InsertTrackAtIndex(0, true)
local tr = reaper.GetTrack(0, 0)
reaper.GetSetMediaTrackInfo_String(tr, "P_NAME", "OpenDeck", true)
local fx = reaper.TrackFX_AddByName(tr, "OpenDeck (Audiodestrukt)", false, -1)
local pitch = -1
for i = 0, reaper.TrackFX_GetNumParams(tr, fx) - 1 do
  local _, name = reaper.TrackFX_GetParamName(tr, fx, i, "")
  if name == "Pitch" then pitch = i end
end
if pitch < 0 then
  reaper.ShowConsoleMsg("OpenDeck: plugin or its Pitch parameter not found (make plugin, then restart REAPER)\n")
  return
end

local env = reaper.GetFXEnvelope(tr, fx, pitch, true)
local function norm(p) return 0.5 + p / 32 end   -- Pitch runs -16..+16 %
local function pt(t, p) reaper.InsertEnvelopePoint(env, t, norm(p), 0, 0, false, true) end
pt(0, 0); pt(6, 0)
pt(14, 8); pt(18, 8)            -- ramp up to +8 %, hold
pt(26, -8); pt(30, -8)          -- ramp down to -8 %, hold
for i = 1, 64 do                -- LFO: +-6 %, 2 s period, 16 s
  pt(30 + i * 0.25, 6 * math.sin(i * 0.25 * math.pi))
end
pt(50, 0)
reaper.Envelope_SortPoints(env)

-- An item gives the project its length (an empty project stops at once).
reaper.CreateNewMIDIItemInProj(tr, 0, 50, false)
reaper.GetSet_LoopTimeRange(true, true, 0, 50, false)
reaper.GetSetRepeat(1)
reaper.TrackList_AdjustWindows(false)
reaper.UpdateArrange()

-- Press play once the audio device is up (a play from a startup script is
-- dropped).
local t0 = reaper.time_precise()
local function later()
  if reaper.time_precise() - t0 > 2 then reaper.OnPlayButton() else reaper.defer(later) end
end
later()
