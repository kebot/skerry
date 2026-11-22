# ARCH-012: H.265 and AV1 Predicted Frames on Selkies 2.3.0 Follow Continuity, Not the Reference Field

- **Status:** settled — amends the `ARCH-003` codec baseline for SealSkin 0.5 / Selkies 2.3.0 servers
- **Finding (QA 2026-11-21):** On SealSkin 0.5.x, H.265 and AV1 streamed at ~1 fps with "Rejected packets" climbing. Packet traces showed that H.264 packets carry `reference` = id of the previous emitted packet, but Selkies 2.3.0's `x265` and `svt-av1` packets carry `reference == id`. `video::Decoder` only accepted a P-frame whose reference equalled the last decoded id, so every H.265/AV1 P-frame was dropped and only the once-per-second recovery keyframes showed.
- **Decision:** `video::follows(last, id, reference)`: when `reference != id`, a predicted frame must reference the last decoded packet (H.264 chaining, unchanged). When `reference == id` (no chain information), it is accepted if the previous frame decoded. The WebSocket is ordered and lossless, and any decode failure still clears `last`, requests a keyframe, and waits for it.
- **Evidence:** live traces on `sealskin.kebot.me` went from H.265 3/22 and AV1 3/26 packets decoded to 21/21 and 26/26; unit test `video::tests::predicted_frames_follow_chained_or_unchained_references`.
- **Affected Contracts:** `src/skerry/video-and-screen.prose.md`, `src/skerry/skerry.test.prose.md`
- **Affected Rust:** `crates/skerry-mac/src/video.rs` (`follows`, `Decoder::decode`)
