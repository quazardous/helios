kind: fixed

The commands plugins bring are now in the app: reading an image's metadata through plugins (`metadata.read`), the camera commands (`device.list`, `device.info`, `device.capture`, `device.download`) and the commands a plugin contributes, such as ExifTool's version, can be run from the palette, menus and agents, each still held to the capabilities the plugin was granted. A plugin slow to start no longer holds up calls to the other plugins, nor the Settings > Plugins page.
