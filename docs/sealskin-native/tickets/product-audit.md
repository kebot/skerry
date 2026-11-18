# SealSkin product comparison

This comparison uses the official SealSkin 0.4.2 documentation, the version previously observed on our server. It covers the native launcher and related workflows. The [Selkies audit](research.md) covers streaming and remote desktop controls. These features were checked against upstream docs and our source. Deployment-specific permissions and workflows still need live testing. [SealSkin usage](https://github.com/selkies-project/sealskin/blob/0.4.2/docs/content/usage.md) · [SealSkin API](https://github.com/selkies-project/sealskin/blob/0.4.2/docs/content/api.md)

SealSkin Native already accepts an application key, lists available apps and sessions, launches an app with server defaults, resumes an individual stream, and disconnects locally. The most visible product gaps are launch options and progress, stopping a session, and opening a link or file in a compatible app. The checklist links to tickets for those and for launcher organization, persistent storage, account recovery, and clustered deployments.

## Product boundaries

- The official collaboration room includes guest invitations, chat, voice/video, controller handoff, gamepad slot assignment, and app switching. The [sharing ticket](sharing.md) covers investigation of that workflow.
- Browser extensions add context menus, media capture, selected-text searches, and Chrome-only download interception. The web app offers bookmarklets, search integration, app links, and installed-app share targets. The [open links and files ticket](open-links-and-files.md) covers the Mac entry points. [Browser entry points](https://github.com/selkies-project/sealskin/blob/0.4.2/docs/content/usage.md#in-a-browser-with-nothing-installed)
- The official dashboard also manages users, groups, sign-in, app stores, templates, PRoot catalogs, VPN profiles, cluster nodes, other users’ sessions, and audit logs. These are server administration features. The native end-user backlog can link to the web dashboard where an account has access. [Administration](https://github.com/selkies-project/sealskin/blob/0.4.2/docs/content/administration.md)
