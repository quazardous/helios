kind: added

Plugins ask before they act: an action of a plugin that lacks a permission it declares (connecting a Google Photos account) opens a dialog listing each permission with the plugin's reason, Allow or Don't allow, instead of failing with "grant it with `plugin.grant`". The answer is remembered in your settings and follows you; Settings > Plugins lists every plugin's permissions with a switch each, and a permission a plugin's update adds is asked for again.
