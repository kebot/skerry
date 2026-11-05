# rb

A native macOS browser for managing remote browsers together with your agent. It shows no local web content: it mirrors the Remote Browser and sends input back, with a local vertical tab sidebar and Tab Groups in the style of Arc and Zen.

## Language

**Remote Browser**:
The Chromium instance running on a server that the app mirrors and controls.
_Avoid_: Server browser, host browser

**Profile**:
A persistent, named Remote Browser identity (cookies, fingerprint, sessions) hosted by a CloakBrowser Manager.
_Avoid_: Account, user, instance

**Manager**:
A CloakBrowser Manager server that hosts Profiles and exposes each one's CDP endpoint behind a Bearer token.
_Avoid_: Server, backend, host

**Agent**:
An AI program that operates a Remote Browser on the user's behalf, alongside the user.
_Avoid_: Bot, assistant, automation, script

**Connection**:
A saved way of reaching a Remote Browser: either a Manager plus Profile, or a raw CDP URL. Each Connection has a colour that marks its Tabs.
_Avoid_: Account, endpoint, session

**Tab**:
A local tab-strip entry that mirrors one remote page target. The Remote Browser owns the real tab.
_Avoid_: Page, window, target

**Placeholder Tab**:
A dimmed Sidebar entry for a Tab of a Connection that is not currently connected, kept from its last-known title and URL so Groups and pins survive.
_Avoid_: Ghost tab, offline tab, stale tab

**Tab Group**:
A named, collapsible, coloured set of Tabs shown together in the Sidebar. A Group may hold Tabs from different Connections. Groups exist only locally; the Remote Browser does not know about them.
_Avoid_: Folder, space, workspace

**Sidebar**:
The vertical list on the left that shows Tabs and Tab Groups from all Connections in one list. It replaces a horizontal tab strip.
_Avoid_: Tab strip, tab bar

**Active Tab**:
The one Tab, across all Connections in the window, whose pixels are currently streamed. Other Tabs are not streamed.
_Avoid_: Current page, focused tab

**Mirror**:
The local view that draws the Active Tab's streamed frames and forwards input to it.
_Avoid_: Viewer, canvas, remote desktop

**Frame**:
One image of the Active Tab received from the Remote Browser.
_Avoid_: Screenshot, snapshot
