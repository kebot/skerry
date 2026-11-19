# Extend codec and rate-control support after H.264

Evaluate H.265, VP8, VP9, and AV1 against the supported Macs and server builds. Expose CBR/CRF, Turbo, paint-over, CPU options, 4:4:4, and 10-bit only for combinations the transport and decoder can play.

[Stream options](https://github.com/selkies-project/selkies/blob/715df7cdea9c6c21ae339f93f194f718a879dc41/docs/settings.md#stream)

## Native support

**Session Controls → Video → Codec** lists JPEG, H.264, H.265 (HEVC), and AV1 according to the server's encoder policy and the Mac’s VideoToolbox capabilities. Selecting a codec reconnects the same session and saves the preference per server. The panel offers JPEG quality or hardware-video CBR target bitrate and CRF quality where the server advertises them. Stream Diagnostics reports the decoder that published a picture.

A disposable Zen session on SealSkin 0.4.2 rendered 1280×720 through each of the four paths on an Apple M5 Pro. H.265 and AV1 used hardware VideoToolbox decoding and GPUI’s full-range NV12 surface path. [Running the app](../running.md)

## Remaining work

VideoToolbox reported no VP8 or VP9 hardware decoder on the test Mac. Those formats still need a compatible decode path. 10-bit and 4:4:4 output, Turbo, and further encoder tuning remain open.

[Selkies web feature research](research.md)
