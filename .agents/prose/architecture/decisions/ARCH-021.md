# ARCH-021: Decoded Frames Are Presented as They Arrive, Not on a 30 Hz Tick

- **Amended by `ARCH-034`:** JPEG frames are presented on their bottom stripe or after a 2 ms settle, as per-stripe images, instead of an 8 ms full-canvas flush.

- **Status:** settled — amends the presentation pacing of `ARCH-003` (`codec-and-rate-control`)
- **Finding (2026-11-21):** the Stats FPS graph (`ARCH-019`) showed the client at 30 fps while the server encoded 60. `transport::run` handed frames to the window only on its 33 ms `present` tick, which the first client (`e0835a5`) used to gather a JPEG frame's stripes into one picture. A hardware frame decoded between ticks waited in `decoded`, and a second one replaced it unseen, so H.264/H.265/AV1 were shown, and counted, at 30 fps at most.
- **Decision:**
  - **Hardware codecs:** each VideoToolbox frame is a whole picture, so it goes to the window (`frames.send_replace`) and is counted the moment it decodes; the `decoded` hold is gone.
  - **JPEG:** stripes still gather into one canvas, now flushed by an 8 ms `jpeg_flush` tick while anything changed, so a 60 fps JPEG stream is not capped at 30. A frame is counted once per frame id, so stripes of one frame split across two flushes do not count twice.
  - The 33 ms tick keeps only what does not need frame rate: `CLIENT_FRAME_ACK` of the newest frame (Selkies needs one every 50 ms), pointer motion, and the hardware-decode expiry check.
- **Affected Contracts:** `src/skerry/video-and-screen.prose.md`, `src/skerry/skerry.test.prose.md`
- **Affected Rust:** `crates/skerry-mac/src/transport.rs`
