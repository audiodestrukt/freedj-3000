-- Helpers for the OpenDeck REAPER scripts (opendeck_mixer.lua,
-- opendeck_routine.lua): decks with their tracks, the crossfader mixer,
-- envelopes, a meter log and a timed quit.
local M = {}

local B64 = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
function M.b64enc(s)
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
function M.b64dec(s)
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

-- Write `path` into the plugin's saved state ({"fields":{"track":"\"<path>\""}}
-- after nih-plug's 8-byte length): the deck loads it.
local function set_track(tr, fx, path)
  local ok, b64 = reaper.TrackFX_GetNamedConfigParm(tr, fx, "clap_chunk")
  local raw = ok and M.b64dec(b64) or nil
  local at = raw and raw:find("{", 1, true)
  if not at then return false end
  local json = raw:sub(at)
  local esc = path:gsub("\\", "\\\\\\\\"):gsub('"', '\\\\\\"')
  json = json:gsub('"fields":%b{}', '"fields":{"track":"\\"' .. esc:gsub("%%", "%%%%") .. '\\""}')
  local prefix = raw:sub(1, at - 1)
  if #prefix == 8 then
    local n, b = #json, {}
    for i = 1, 8 do b[i] = string.char(n % 256); n = n // 256 end
    prefix = table.concat(b)
  end
  return reaper.TrackFX_SetNamedConfigParm(tr, fx, "clap_chunk", M.b64enc(prefix .. json))
end

-- A track named `name` with OpenDeck on it, loading `path`, not sent to the
-- master.  Returns {tr, fx, param = {name -> index}}.
function M.deck(idx, name, path)
  reaper.InsertTrackAtIndex(idx, true)
  local tr = reaper.GetTrack(0, idx)
  reaper.GetSetMediaTrackInfo_String(tr, "P_NAME", name, true)
  local fx = reaper.TrackFX_AddByName(tr, "CLAPi: OpenDeck (Audiodestrukt)", false, -1)
  if fx < 0 then
    reaper.ShowConsoleMsg("OpenDeck: plugin not found (make plugin, then restart REAPER)\n")
    return nil
  end
  if path and path ~= "" and not set_track(tr, fx, path) then
    reaper.ShowConsoleMsg("OpenDeck: could not write the plugin's state; " .. name .. " boots empty\n")
  end
  reaper.SetMediaTrackInfo_Value(tr, "B_MAINSEND", 0)
  local param = {}
  for i = 0, reaper.TrackFX_GetNumParams(tr, fx) - 1 do
    local _, pn = reaper.TrackFX_GetParamName(tr, fx, i, "")
    param[pn] = i
  end
  return { tr = tr, fx = fx, param = param }
end

-- The "Mixer" track: opendeck_xfader.jsfx with deck a on 1/2 and b on 3/4.
function M.mixer(idx, a, b)
  reaper.InsertTrackAtIndex(idx, true)
  local tr = reaper.GetTrack(0, idx)
  reaper.GetSetMediaTrackInfo_String(tr, "P_NAME", "Mixer", true)
  reaper.SetMediaTrackInfo_Value(tr, "I_NCHAN", 4)
  local fx = reaper.TrackFX_AddByName(tr, "JS:OpenDeck/opendeck_xfader.jsfx", false, -1)
  if fx < 0 then
    reaper.ShowConsoleMsg("OpenDeck: crossfader JSFX not found (the make target installs it)\n")
    return nil
  end
  local sa = reaper.CreateTrackSend(a.tr, tr)
  reaper.SetTrackSendInfo_Value(a.tr, 0, sa, "I_DSTCHAN", 0)
  local sb = reaper.CreateTrackSend(b.tr, tr)
  reaper.SetTrackSendInfo_Value(b.tr, 0, sb, "I_DSTCHAN", 2)
  return { tr = tr, fx = fx, param = { xf = 0, curve = 1, fa = 2, fb = 3 } }
end

-- An envelope on parameter `p` of `t` (a deck or the mixer), with points
-- {time, value(normalised)} and a shape (0 linear, 1 square).
function M.envelope(t, p, points, shape)
  local env = reaper.GetFXEnvelope(t.tr, t.fx, p, true)
  for _, q in ipairs(points) do reaper.InsertEnvelopePoint(env, q[1], q[2], shape or 0, 0, false, true) end
  reaper.Envelope_SortPoints(env)
  return env
end

-- Give the project its length, loop it, and press play once the audio
-- device is up (a play from a startup script is dropped).
function M.play(len, on_track)
  reaper.CreateNewMIDIItemInProj(on_track, 0, len, false)
  reaper.GetSet_LoopTimeRange(true, true, 0, len, false)
  reaper.GetSetRepeat(1)
  reaper.TrackList_AdjustWindows(false)
  reaper.UpdateArrange()
  local t0 = reaper.time_precise()
  local function later()
    if reaper.time_precise() - t0 > 2 then reaper.OnPlayButton() else reaper.defer(later) end
  end
  later()
  return t0
end

-- OPENDECK_MIXER_LOG=file: the play position, the crossfader and each
-- track's peak once a second.  OPENDECK_QUIT_AFTER=secs: quit REAPER then.
function M.monitor(t0, a, b, mix)
  local logf, quit = os.getenv("OPENDECK_MIXER_LOG"), tonumber(os.getenv("OPENDECK_QUIT_AFTER") or "")
  if not (logf and logf ~= "") and not quit then return end
  local last = 0
  local function pk(tr) return math.max(reaper.Track_GetPeakInfo(tr, 0), reaper.Track_GetPeakInfo(tr, 1)) end
  local function tick()
    local now = reaper.time_precise()
    if quit and now - t0 > quit then reaper.OnStopButton(); reaper.Main_OnCommand(40004, 0); return end
    if logf and logf ~= "" and now - last >= 1 then
      last = now
      local f = io.open(logf, "a")
      f:write(string.format("t=%6.1f pos=%6.1f xf=%.2f A=%.3f B=%.3f mix=%.3f\n", now - t0, reaper.GetPlayPosition(),
        reaper.TrackFX_GetParam(mix.tr, mix.fx, 0), pk(a.tr), pk(b.tr), pk(mix.tr)))
      f:close()
    end
    reaper.defer(tick)
  end
  tick()
end

return M
