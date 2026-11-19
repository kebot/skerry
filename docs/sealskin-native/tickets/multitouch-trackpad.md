# Offer multitouch trackpad forwarding

**Forward Multitouch Trackpad** starts off and only appears in **Session Controls → Input** when the server advertises support. The app forwards snapshots of indirect AppKit contacts with stable IDs and normalized coordinates. An empty snapshot ends active contacts. The app clears them on focus loss or when the option is turned off. The server also clears the owning connection’s contacts on disconnect.

## Server support

Apply the [Selkies server patch](../../../patches/selkies/multitouch-trackpad.patch) to upstream commit `715df7cdea9c6c21ae339f93f194f718a879dc41`. Rebuild Selkies and set `SELKIES_MULTITOUCH_TRACKPAD=true` for a test X11 session. The server needs write access to `/dev/uinput`, and Xorg must discover the new input device through its normal Linux input seat. Selkies advertises the capability after device creation succeeds. Its embedded Wayland compositor needs a separate input backend before it can advertise the option.

The server accepts full snapshots as `mt,[[id,x,y],…]`, with no more than ten contacts and coordinates from 0 to 32767. Authenticated primary controllers can claim the device. The server rejects malformed snapshots and prevents a second connection from moving or lifting the current owner’s contacts.

## Hands-on verification

Enable **Forward Multitouch Trackpad** on the patched test server. Try a two-finger pan, pinch, and a gesture with three fingers supported by that Linux session. Confirm that lifted fingers, hiding or closing the window, and reconnecting release active contacts. Disable the option and check that standard Mac pinch and pan work again. macOS still owns gestures it reserves before AppKit delivers the contacts.

[AppKit touch input](https://developer.apple.com/documentation/appkit/nstouch) · [Linux multitouch protocol](https://docs.kernel.org/input/multi-touch-protocol.html) · [Linux virtual input](https://docs.kernel.org/input/uinput.html)
