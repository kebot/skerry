# Scale and letterbox Frames; never override the viewport

The Mirror scales and letterboxes each Frame to fit its area, and maps input coordinates back through the same scale. We do not call `Emulation.setDeviceMetricsOverride` to match the remote viewport to the local window. Profiles are anti-detect with a fixed fingerprint (`screen_width`/`screen_height`), and an override could produce detectable mismatches between the reported screen and the viewport. It would also change the page for the Agent and for any VNC viewers attached to the same Remote Browser.

**Cost:** Frames are resampled, so the Mirror is somewhat less sharp than a native-resolution view. Revisit if blur bothers users; any alternative must be shown not to alter the fingerprint or the Agent's view.
