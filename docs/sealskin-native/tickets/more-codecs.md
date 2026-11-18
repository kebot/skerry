# Extend codec and rate-control support after H.264

Evaluate H.265, VP8, VP9, and AV1 against the supported Macs and server builds. Expose CBR/CRF, Turbo, paint-over, CPU options, 4:4:4, and 10-bit only for combinations the transport and decoder can play.

[Stream options](https://github.com/selkies-project/selkies/blob/715df7cdea9c6c21ae339f93f194f718a879dc41/docs/settings.md#stream)

## Probe results

The disposable Zen session sent H.265, VP8, VP9, and AV1 as full-frame video over WebSockets. FFmpeg with VideoToolbox requested decoded the captured HEVC Main and AV1 Main samples on the test Mac. The samples were 1280×720, 8-bit 4:2:0. VP8 and VP9 decode, 10-bit, 4:4:4, and other Mac models still need verification.

[Selkies web feature research](research.md)
