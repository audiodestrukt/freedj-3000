-- A beat-juggling and scratch routine, all automation, on two OpenDeck decks.
--
--   OPENDECK_TRACK=a.mp3 OPENDECK_BPM=130 reaper -new tools/reaper/opendeck_routine.lua
--   (or: make reaper-routine TRACK=a.mp3 BPM=130)
--
-- Both decks load the same track and sit at its cue (AUTO CUE: the first
-- beat).  The crossfader (opendeck_xfader.jsfx) is on its Cut curve: 0 = A
-- only, 1 = B only, anywhere between = both, as on a scratch mixer.
--
--   bars 1-4    deck A plays the intro
--   bars 5-8    loop juggle on the first beats, A and B taking turns, each
--               back-cued (CUE while playing) as the other plays:
--               half-bars, then beats, then half-beats
--   bars 9-12   A plays; B scratches over it from its cue:
--               baby scratches, a transformer (crossfader cuts), chirps
--   bars 13-16  B is let go and plays; over to B; A stops
--   end         both back to their cues; the project loops
--
-- Every move is an envelope on the deck's plugin parameters (Play, Cue,
-- Platter touch, Platter) or the crossfader, so it can be edited in REAPER.
local here = debug.getinfo(1, "S").source:match("^@(.*[/\\])") or "./"
local L = dofile(here .. "opendeck_lib.lua")

local bpm = tonumber(os.getenv("OPENDECK_BPM") or "") or 130
local B = 60 / bpm                       -- one beat, in seconds
local function at(beat) return beat * B end
local track = os.getenv("OPENDECK_TRACK")
local a = L.deck(0, "Deck A", track)
local b = L.deck(1, "Deck B", os.getenv("OPENDECK_TRACK_B") or track)
if not (a and b) then return end
local mix = L.mixer(2, a, b)
if not mix then return end
reaper.SetCurrentBPM(0, bpm, false)
reaper.TrackFX_SetParam(mix.tr, mix.fx, mix.param.curve, 2)    -- Cut

-- Envelope point lists, filled below, written at the end.
local ev = { a = { play = {}, cue = {}, touch = {}, platter = {} },
             b = { play = {}, cue = {}, touch = {}, platter = {} }, xf = {} }
local function sq(list, t, v) list[#list + 1] = { t, v } end
local PLATTER = 8                                    -- the Platter parameter's reach (s)
local function plat(v) return (v + PLATTER) / (2 * PLATTER) end

-- The buttons need ~12 ms between edges (REAPER reads parameters once a
-- block, 11.6 ms at 512 frames).  A back-cue is CUE pressed while playing,
-- released, then Play off: in that order, or CUE on a paused deck would set
-- a new cue point.
local GAP = 0.020
local function start(d, t) sq(ev[d].play, t, 1) end
local function backcue(d, t)                         -- done by time t
  sq(ev[d].cue, t - 3 * GAP, 1); sq(ev[d].cue, t - 2 * GAP, 0); sq(ev[d].play, t - GAP, 0)
end
local function xf(t, v) sq(ev.xf, t, v) end

-- Everything off at the top.
for _, d in ipairs({ "a", "b" }) do
  sq(ev[d].play, 0, 0); sq(ev[d].cue, 0, 0); sq(ev[d].touch, 0, 0); sq(ev[d].platter, 0, plat(0))
end
xf(0, 0)

-- Bars 1-4: A plays the intro.
start("a", at(0))

-- Bars 5-8: loop juggle.  `len` beats per turn, B first.
local beat, who = 16, "b"
backcue("a", at(16))
for _, part in ipairs({ { len = 2, beats = 8 }, { len = 1, beats = 4 }, { len = 0.5, beats = 4 } }) do
  local stop = beat + part.beats
  while beat < stop - 1e-6 do
    xf(at(beat), who == "a" and 0 or 1)
    start(who, at(beat))
    backcue(who, at(beat + part.len))
    beat = beat + part.len
    who = who == "a" and "b" or "a"
  end
end

-- Bars 9-12: A plays, B scratches over it from its cue.
start("a", at(32))
xf(at(32), 0)
sq(ev.b.touch, at(32), 1)
local L_STROKE = 0.30                                 -- seconds of record per stroke
local function stroke(t0, t1, p0, p1) sq(ev.b.platter, at(t0), plat(p0)); sq(ev.b.platter, at(t1), plat(p1)) end
-- Bars 9-10: baby scratches, one push and pull a beat, fader open.
xf(at(32) + 0.001, 0.5)
for k = 0, 7 do stroke(32 + k, 32 + k + 0.5, 0, L_STROKE); stroke(32 + k + 0.5, 32 + k + 1, L_STROKE, 0) end
-- Bar 11: transformer: the same strokes, the fader cutting B in and out
-- every eighth of a beat.
for k = 0, 3 do stroke(40 + k, 40 + k + 0.5, 0, L_STROKE); stroke(40 + k + 0.5, 40 + k + 1, L_STROKE, 0) end
for i = 0, 31 do xf(at(40 + i / 8), i % 2 == 0 and 0.5 or 0) end
-- Bar 12: chirps: open on the push, closed at its peak, open again as the
-- pull lands.
for k = 0, 3 do
  local t = 44 + k
  xf(at(t), 0.5); stroke(t, t + 0.4, 0, L_STROKE)
  xf(at(t + 0.4), 0); stroke(t + 0.4, t + 0.9, L_STROKE, 0)
end
xf(at(48) - 0.01, 0.5)

-- Bars 13-16: B is let go and plays on; over to B; A stops.
sq(ev.b.touch, at(48), 0)
start("b", at(48))
xf(at(52), 1)
backcue("a", at(56))

-- End: both back to their cues for the next loop.
local len = at(64)
backcue("b", len - 0.01)
xf(len - 0.01, 0)

-- Write the envelopes: buttons square, the platter and the fader linear
-- (the fader's cuts are steps anyway: their points are close together).
for _, d in ipairs({ "a", "b" }) do
  local t = ({ a = a, b = b })[d]
  L.envelope(t, t.param["Play"], ev[d].play, 1)
  L.envelope(t, t.param["Cue"], ev[d].cue, 1)
  L.envelope(t, t.param["Platter touch"], ev[d].touch, 1)
  L.envelope(t, t.param["Platter"], ev[d].platter, 0)
end
L.envelope(mix, mix.param.xf, ev.xf, 1)

local t0 = L.play(len, mix.tr)
L.monitor(t0, a, b, mix)
