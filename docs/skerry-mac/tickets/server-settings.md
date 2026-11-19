# Model the capabilities announced by each session

Parse `server_settings` and relevant updates for current values, server defaults, allowed choices, ranges, and locks. Use the intersection of server capabilities and native support. Resolve saved preferences on connect and reconnect. Update the UI when the server changes or rejects a choice. Preserve working JPEG defaults for older builds that omit metadata. Test locked, ranged, unsupported, and missing values.

[Selkies settings syntax](https://github.com/selkies-project/selkies/blob/715df7cdea9c6c21ae339f93f194f718a879dc41/docs/settings.md#value-syntax) · [Dashboard settings state](https://github.com/selkies-project/selkies/blob/715df7cdea9c6c21ae339f93f194f718a879dc41/web/selkies-dashboard/src/state/settings.ts)

[Selkies web feature research](research.md)
