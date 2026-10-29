# Real-time priority for the audio threads

On a one-core board (Pi Zero W) the render loop and the Link threads share
the CPU with the audio processor, and the ring starves whenever the scheduler
wakes it late — measured as 10–57 s of silence per minute on the Zero W.
OpenDeck asks for `SCHED_FIFO` on `audio-proc` (20) and the cpal device
thread (30) at start, which needs an rtprio limit the stock Pi OS only gives
the `pipewire` group.  Install this once (the user is in `audio` already):

```bash
sudo cp deploy/pi/rt-audio/99-opendeck-audio.conf /etc/security/limits.d/
# then log out and back in (or reboot) so the new limits apply
ulimit -r     # should print 80
```

The log says which it got: `audio-proc: SCHED_FIFO priority 20` or
`real-time priority not permitted`.  `OPENDECK_RT=0` leaves priorities alone;
`OPENDECK_RING_MS=200` decodes further ahead instead (the ring is flushed on
every seek, so a longer one does not delay CUE).
