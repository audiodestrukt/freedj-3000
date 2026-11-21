-- Two OpenDeck decks into a DJ crossfader in REAPER.
--
--   OPENDECK_TRACK_A=a.mp3 OPENDECK_TRACK_B=b.mp3 OPENDECK_AUTOPLAY=1 \
--     reaper -new tools/reaper/opendeck_mixer.lua          (or: make reaper-mixer)
--
-- Tracks: "Deck A" and "Deck B" (OpenDeck each, sent to the mixer, not to the
-- master) and "Mixer" (tools/reaper/opendeck_xfader.jsfx: A on 1/2, B on 3/4).
-- The crossfader is automated: A for 20 s, over to B in 8 s, B for 20 s, back
-- in 8 s, looped.  Each deck's track is written into the plugin's saved state
-- (its "track" field), so each deck loads its own, and the project keeps them.

local function deck(idx, name, path)
  reaper.InsertTrackAtIndex(idx, true)
  local tr = reaper.GetTrack(0, idx)
  reaper.GetSetMediaTrackInfo_String(tr, "P_NAME", name, true)
  local fx = reaper.TrackFX_AddByName(tr, "CLAPi: OpenDeck (Audiodestrukt)", false, -1)
  if fx < 0 then
    reaper.ShowConsoleMsg("OpenDeck: plugin not found (make plugin, then restart REAPER)\n")
    return nil
  end
  if path and path ~= "" then
    -- The plugin's state is JSON ({"fields":{"track":"\"<path>\""}...}) after
    -- an 8-byte length; set the track field and fix the length.
    local ok, b64 = reaper.TrackFX_GetNamedConfigParm(tr, fx, "clap_chunk")
    local raw = ok and base64dec(b64) or nil
    local at = raw and raw:find("{", 1, true)
    if at then
      local json = raw:sub(at)
      local esc = path:gsub("\\", "\\\\\\\\"):gsub('"', '\\\\\\"')
      json = json:gsub('"fields":%b{}', '"fields":{"track":"\\"' .. esc:gsub("%%", "%%%%") .. '\\""}')
      local prefix = raw:sub(1, at - 1)
      if #prefix == 8 then
        local n, b = #json, {}
        for i = 1, 8 do b[i] = string.char(n % 256); n = n // 256 end
        prefix = table.concat(b)
      end
      reaper.TrackFX_SetNamedConfigParm(tr, fx, "clap_chunk", base64enc(prefix .. json))
    else
      reaper.ShowConsoleMsg("OpenDeck: could not read the plugin's state; " .. name .. " boots empty\n")
    end
  end
  reaper.SetMediaTrackInfo_Value(tr, "B_MAINSEND", 0)
  return tr
end

local B64 = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
function base64enc(s)
  return ((s:gsub(".", function(c)
    local r, b = "", c:byte()
    for i = 8, 1, -1 do r = r .. (b % 2 ^ i - b % 2 ^ (i - 1) > 0 and "1" or "0") end
    return r
  end) .. "0000"):gsub("%d%d%d?%d?%d?%d?", function(x)
    if #x < 6 then return "" end
    local c = 0
    for i = 1, 6 do c = c + (x:sub(i, i) == "1" and 2 ^ (6 - i) or 0) end
    return B64:sub(c + 1, c + 1)
  end) .. ({ "", "==", "=" })[#s % 3 + 1])
end
function base64dec(s)
  s = s:gsub("[^" .. B64 .. "=]", "")
  return (s:gsub(".", function(x)
    if x == "=" then return "" end
    local r, f = "", (B64:find(x, 1, true) - 1)
    for i = 6, 1, -1 do r = r .. (f % 2 ^ i - f % 2 ^ (i - 1) > 0 and "1" or "0") end
    return r
  end):gsub("%d%d%d?%d?%d?%d?%d?%d?", function(x)
    if #x ~= 8 then return "" end
    local c = 0
    for i = 1, 8 do c = c + (x:sub(i, i) == "1" and 2 ^ (8 - i) or 0) end
    return string.char(c)
  end))
end

local a = deck(0, "Deck A", os.getenv("OPENDECK_TRACK_A"))
local b = deck(1, "Deck B", os.getenv("OPENDECK_TRACK_B"))
if not (a and b) then return end

reaper.InsertTrackAtIndex(2, true)
local mix = reaper.GetTrack(0, 2)
reaper.GetSetMediaTrackInfo_String(mix, "P_NAME", "Mixer", true)
reaper.SetMediaTrackInfo_Value(mix, "I_NCHAN", 4)
local xf = reaper.TrackFX_AddByName(mix, "JS:OpenDeck/opendeck_xfader.jsfx", false, -1)
if xf < 0 then
  reaper.ShowConsoleMsg("OpenDeck: crossfader JSFX not found (make reaper-mixer installs it)\n")
  return
end
local sa = reaper.CreateTrackSend(a, mix)
reaper.SetTrackSendInfo_Value(a, 0, sa, "I_DSTCHAN", 0)     -- A -> 1/2
local sb = reaper.CreateTrackSend(b, mix)
reaper.SetTrackSendInfo_Value(b, 0, sb, "I_DSTCHAN", 2)     -- B -> 3/4

local env = reaper.GetFXEnvelope(mix, xf, 0, true)
local function pt(t, v) reaper.InsertEnvelopePoint(env, t, v, 0, 0, false, true) end
pt(0, 0); pt(20, 0); pt(28, 1); pt(48, 1); pt(56, 0)
reaper.Envelope_SortPoints(env)

-- An item gives the project its length (an empty project stops at once).
reaper.CreateNewMIDIItemInProj(mix, 0, 56, false)
reaper.GetSet_LoopTimeRange(true, true, 0, 56, false)
reaper.GetSetRepeat(1)
reaper.TrackList_AdjustWindows(false)
reaper.UpdateArrange()

-- Play once the audio device is up (a play from a startup script is dropped).
local t0 = reaper.time_precise()
local function later()
  if reaper.time_precise() - t0 > 2 then reaper.OnPlayButton() else reaper.defer(later) end
end
later()

-- OPENDECK_MIXER_LOG=file: log the play position, the crossfader and each
-- track's peak once a second (to check a run without listening).
local logf = os.getenv("OPENDECK_MIXER_LOG")
if logf and logf ~= "" then
  local last = 0
  local function meter()
    local now = reaper.time_precise()
    if now - last >= 1 then
      last = now
      local f = io.open(logf, "a")
      local x = reaper.TrackFX_GetParam(mix, xf, 0)
      local function pk(tr) return math.max(reaper.Track_GetPeakInfo(tr, 0), reaper.Track_GetPeakInfo(tr, 1)) end
      f:write(string.format("t=%6.1f pos=%6.1f xf=%.2f A=%.3f B=%.3f mix=%.3f\n", now - t0, reaper.GetPlayPosition(), x, pk(a), pk(b), pk(mix)))
      f:close()
    end
    reaper.defer(meter)
  end
  meter()
end
