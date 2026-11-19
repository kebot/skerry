# Revisit raw multitouch trackpad forwarding

Raw contact forwarding was removed after hands-on testing found the latency too high for Chrome. SealSkin continues to use native macOS pinch and pan with the existing Selkies protocol.

Before revisiting the feature, measure gesture-to-display latency against native pinch and pan at the same codec, resolution, and frame rate. The earlier X11 prototype verified contact tracking and release through a virtual Linux trackpad. A future implementation should ship as an opt-in only after verifying usable latency in Chrome and reliable contact release on focus loss and reconnect.
