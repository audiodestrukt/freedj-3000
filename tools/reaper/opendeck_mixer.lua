-- Two OpenDeck decks into a DJ crossfader in REAPER.
--
--   OPENDECK_TRACK_A=a.mp3 OPENDECK_TRACK_B=b.mp3 OPENDECK_AUTOPLAY=1 \
--     reaper -new tools/reaper/opendeck_mixer.lua          (or: make reaper-mixer)
--
-- Tracks: "Deck A" and "Deck B" (OpenDeck each, sent to the mixer, not to the
-- master) and "Mixer" (tools/reaper/opendeck_xfader.jsfx: A on 1/2, B on 3/4).
-- The crossfader is automated: A for 20 s, over to B in 8 s, B for 20 s, back
-- in 8 s, looped.  Each deck's track is written into the plugin's saved state,
-- so each deck loads its own, and the project keeps them.
local here = debug.getinfo(1, "S").source:match("^@(.*[/\\])") or "./"
local L = dofile(here .. "opendeck_lib.lua")

local a = L.deck(0, "Deck A", os.getenv("OPENDECK_TRACK_A"))
local b = L.deck(1, "Deck B", os.getenv("OPENDECK_TRACK_B"))
if not (a and b) then return end
local mix = L.mixer(2, a, b)
if not mix then return end

L.envelope(mix, mix.param.xf, { { 0, 0 }, { 20, 0 }, { 28, 1 }, { 48, 1 }, { 56, 0 } })
local t0 = L.play(56, mix.tr)
L.monitor(t0, a, b, mix)
